//! Stage 3 durable state: 10-minute one-time pairing tickets, paired devices,
//! Agent Mesh nodes, and private notification channels.
use crate::{decode_enum, enum_value, new_id, now, parse_json, Error, Result, Store};
use polyclave_proto::{
    ChangeTopic, ClientKind, MeshNodeDto, MeshNodeStatus, NotificationChannelDto,
    NotificationChannelKind, PairedDeviceDto, PairingTicketDto,
};
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

/// Additive, idempotent Stage 3 schema.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS pairing_tickets (
  id           TEXT PRIMARY KEY,
  ticket_hash  TEXT NOT NULL UNIQUE,
  client_kind  TEXT NOT NULL CHECK (client_kind IN ('desktop', 'web', 'ios', 'android', 'cli')),
  device_label TEXT NOT NULL,
  expires_at   TEXT NOT NULL,
  used_at      TEXT,
  created_at   TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS paired_devices (
  id            TEXT PRIMARY KEY,
  label         TEXT NOT NULL,
  client_kind   TEXT NOT NULL CHECK (client_kind IN ('desktop', 'web', 'ios', 'android', 'cli')),
  peer_identity TEXT,
  token_hash    TEXT NOT NULL UNIQUE,
  last_seen_at  TEXT,
  revoked_at    TEXT,
  created_at    TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS mesh_nodes (
  id                   TEXT PRIMARY KEY,
  name                 TEXT NOT NULL,
  endpoint             TEXT NOT NULL,
  os_family            TEXT NOT NULL,
  allowed_repos_json   TEXT NOT NULL DEFAULT '[]',
  allowed_actions_json TEXT NOT NULL DEFAULT '[]',
  status               TEXT NOT NULL CHECK (status IN ('online', 'offline', 'revoked')),
  last_heartbeat_at    TEXT,
  created_at           TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS notification_channels (
  id         TEXT PRIMARY KEY,
  kind       TEXT NOT NULL CHECK (kind IN ('ntfy', 'unified_push')),
  label      TEXT NOT NULL,
  endpoint   TEXT NOT NULL,
  topic      TEXT NOT NULL,
  enabled    INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  created_at TEXT NOT NULL
);
";

#[derive(Clone, Debug)]
pub struct NewMeshNode {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub os_family: String,
    pub allowed_repos: Vec<String>,
    pub allowed_actions: Vec<String>,
    pub status: MeshNodeStatus,
}

#[derive(Clone, Debug)]
pub struct NewNotificationChannel {
    pub id: Option<String>,
    pub kind: NotificationChannelKind,
    pub label: String,
    pub endpoint: String,
    pub topic: String,
    pub enabled: bool,
}

fn hash_secret(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn read_ticket(db: &Connection, id: &str) -> Result<PairingTicketDto> {
    db.query_row(
        "SELECT id, client_kind, device_label, expires_at, used_at, created_at
         FROM pairing_tickets WHERE id=?1",
        [id],
        |r| {
            let kind: String = r.get(1)?;
            Ok(PairingTicketDto {
                id: r.get(0)?,
                client_kind: decode_enum(&kind)?,
                device_label: r.get(2)?,
                expires_at: r.get(3)?,
                used_at: r.get(4)?,
                created_at: r.get(5)?,
            })
        },
    )
    .optional()?
    .ok_or(Error::NotFound)
}

fn read_device(db: &Connection, id: &str) -> Result<PairedDeviceDto> {
    db.query_row(
        "SELECT id, label, client_kind, peer_identity, last_seen_at, revoked_at, created_at
         FROM paired_devices WHERE id=?1",
        [id],
        |r| {
            let kind: String = r.get(2)?;
            Ok(PairedDeviceDto {
                id: r.get(0)?,
                label: r.get(1)?,
                client_kind: decode_enum(&kind)?,
                peer_identity: r.get(3)?,
                last_seen_at: r.get(4)?,
                revoked_at: r.get(5)?,
                created_at: r.get(6)?,
            })
        },
    )
    .optional()?
    .ok_or(Error::NotFound)
}

fn read_mesh_node(db: &Connection, id: &str) -> Result<MeshNodeDto> {
    db.query_row(
        "SELECT id, name, endpoint, os_family, allowed_repos_json, allowed_actions_json,
                status, last_heartbeat_at, created_at
         FROM mesh_nodes WHERE id=?1",
        [id],
        |r| {
            let status: String = r.get(6)?;
            Ok(MeshNodeDto {
                id: r.get(0)?,
                name: r.get(1)?,
                endpoint: r.get(2)?,
                os_family: r.get(3)?,
                allowed_repos: parse_json(r.get(4)?, 4)?,
                allowed_actions: parse_json(r.get(5)?, 5)?,
                status: decode_enum(&status)?,
                last_heartbeat_at: r.get(7)?,
                created_at: r.get(8)?,
            })
        },
    )
    .optional()?
    .ok_or(Error::NotFound)
}

fn read_channel(db: &Connection, id: &str) -> Result<NotificationChannelDto> {
    db.query_row(
        "SELECT id, kind, label, endpoint, topic, enabled, created_at
         FROM notification_channels WHERE id=?1",
        [id],
        |r| {
            let kind: String = r.get(1)?;
            Ok(NotificationChannelDto {
                id: r.get(0)?,
                kind: decode_enum(&kind)?,
                label: r.get(2)?,
                endpoint: r.get(3)?,
                topic: r.get(4)?,
                enabled: r.get::<_, i64>(5)? != 0,
                created_at: r.get(6)?,
            })
        },
    )
    .optional()?
    .ok_or(Error::NotFound)
}

impl Store {
    /// Creates a single-use pairing ticket valid for `ttl_secs` (clamped 60..=600)
    /// and returns `(dto, one_time_plaintext_secret)`. Only the SHA-256 hash is stored.
    pub fn create_pairing_ticket(
        &self,
        client_kind: ClientKind,
        device_label: impl Into<String>,
        ttl_secs: i64,
    ) -> Result<(PairingTicketDto, String)> {
        let label = device_label.into();
        if label.trim().is_empty() {
            return Err(Error::Invalid("device_label must not be empty"));
        }
        let ttl = ttl_secs.clamp(60, 600);
        let secret = uuid::Uuid::new_v4().simple().to_string();
        let ticket_hash = hash_secret(&secret);
        let id = new_id('p');
        let kind_str = enum_value(client_kind)?;
        let dto = self.write(move |db| {
            let ts = now(db)?;
            let expires: String = db.query_row(
                "SELECT strftime('%Y-%m-%dT%H:%M:%fZ', 'now', ?1)",
                [format!("+{ttl} seconds")],
                |r| r.get(0),
            )?;
            db.execute(
                "INSERT INTO pairing_tickets (id, ticket_hash, client_kind, device_label, expires_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, ticket_hash, kind_str, label.trim(), expires, ts],
            )?;
            read_ticket(db, &id)
        })?;
        Ok((dto, secret))
    }

    pub fn list_pairing_tickets(&self) -> Result<Vec<PairingTicketDto>> {
        self.read(|db| {
            let mut stmt = db.prepare(
                "SELECT id, client_kind, device_label, expires_at, used_at, created_at
                 FROM pairing_tickets ORDER BY rowid DESC LIMIT 50",
            )?;
            let rows = stmt.query_map([], |r| {
                let kind: String = r.get(1)?;
                Ok(PairingTicketDto {
                    id: r.get(0)?,
                    client_kind: decode_enum(&kind)?,
                    device_label: r.get(2)?,
                    expires_at: r.get(3)?,
                    used_at: r.get(4)?,
                    created_at: r.get(5)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Error::from)
        })
    }

    /// Exchanges a valid, unused, unexpired pairing ticket for a long-lived
    /// device bearer token (returned once; only its SHA-256 hash is persisted).
    pub fn exchange_pairing_ticket(
        &self,
        ticket_secret: &str,
        peer_identity: Option<String>,
    ) -> Result<(PairedDeviceDto, String)> {
        if ticket_secret.trim().is_empty() {
            return Err(Error::Invalid("ticket secret required"));
        }
        let ticket_hash = hash_secret(ticket_secret.trim());
        let raw_token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        let token_hash = hash_secret(&raw_token);
        let device_id = new_id('d');
        let dto = self.write(move |db| {
            let ts = now(db)?;
            let row: Option<(String, String, String, String, Option<String>)> = db
                .query_row(
                    "SELECT id, client_kind, device_label, expires_at, used_at
                     FROM pairing_tickets WHERE ticket_hash=?1",
                    [&ticket_hash],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .optional()?;
            let Some((ticket_id, kind_str, label, expires_at, used_at)) = row else {
                return Err(Error::NotFound);
            };
            if used_at.is_some() || expires_at <= ts {
                return Err(Error::ConflictError);
            }
            let updated = db.execute(
                "UPDATE pairing_tickets SET used_at=?1 WHERE id=?2 AND used_at IS NULL",
                params![ts, ticket_id],
            )?;
            if updated != 1 {
                return Err(Error::ConflictError);
            }
            let peer = peer_identity
                .map(|p| p.trim().to_owned())
                .filter(|p| !p.is_empty());
            db.execute(
                "INSERT INTO paired_devices (id, label, client_kind, peer_identity, token_hash, last_seen_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![device_id, label, kind_str, peer, token_hash, ts],
            )?;
            let dto = read_device(db, &device_id)?;
            Store::append_change(
                db,
                ChangeTopic::Device,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })?;
        Ok((dto, raw_token))
    }

    /// Verifies a device bearer token and optional remote peer identity
    /// (`Tailscale-User-Login` / `X-AgentMesh-Node`). Rejects revoked or mismatched devices.
    pub fn verify_device_token(
        &self,
        token: &str,
        peer_identity: Option<&str>,
    ) -> Result<Option<PairedDeviceDto>> {
        let token_hash = hash_secret(token);
        let peer_owned = peer_identity.map(str::to_owned);
        self.write(move |db| {
            let row: Option<(String, Option<String>, Option<String>)> = db
                .query_row(
                    "SELECT id, peer_identity, revoked_at FROM paired_devices WHERE token_hash=?1",
                    [&token_hash],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((id, expected_peer, revoked_at)) = row else {
                return Ok(None);
            };
            if revoked_at.is_some() {
                return Ok(None);
            }
            if let Some(expected) = expected_peer {
                if peer_owned.as_deref() != Some(expected.as_str()) {
                    return Ok(None);
                }
            }
            let ts = now(db)?;
            db.execute(
                "UPDATE paired_devices SET last_seen_at=?1 WHERE id=?2",
                params![ts, id],
            )?;
            Ok(Some(read_device(db, &id)?))
        })
    }

    pub fn list_paired_devices(&self) -> Result<Vec<PairedDeviceDto>> {
        self.read(|db| {
            let mut stmt = db.prepare(
                "SELECT id, label, client_kind, peer_identity, last_seen_at, revoked_at, created_at
                 FROM paired_devices ORDER BY rowid DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                let kind: String = r.get(2)?;
                Ok(PairedDeviceDto {
                    id: r.get(0)?,
                    label: r.get(1)?,
                    client_kind: decode_enum(&kind)?,
                    peer_identity: r.get(3)?,
                    last_seen_at: r.get(4)?,
                    revoked_at: r.get(5)?,
                    created_at: r.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Error::from)
        })
    }

    pub fn revoke_paired_device(&self, id: &str) -> Result<PairedDeviceDto> {
        let id = id.to_owned();
        self.write(move |db| {
            let ts = now(db)?;
            let changed = db.execute(
                "UPDATE paired_devices SET revoked_at=COALESCE(revoked_at, ?1) WHERE id=?2",
                params![ts, id],
            )?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            let dto = read_device(db, &id)?;
            Store::append_change(
                db,
                ChangeTopic::Device,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn upsert_mesh_node(&self, input: NewMeshNode) -> Result<MeshNodeDto> {
        if input.id.trim().is_empty() || input.name.trim().is_empty() {
            return Err(Error::Invalid("mesh node id and name are required"));
        }
        let repos_json = serde_json::to_string(&input.allowed_repos)?;
        let actions_json = serde_json::to_string(&input.allowed_actions)?;
        let status_str = enum_value(input.status)?;
        self.write(move |db| {
            let ts = now(db)?;
            db.execute(
                "INSERT INTO mesh_nodes (id, name, endpoint, os_family, allowed_repos_json, allowed_actions_json, status, last_heartbeat_at, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)
                 ON CONFLICT(id) DO UPDATE SET
                   name=excluded.name,
                   endpoint=excluded.endpoint,
                   os_family=excluded.os_family,
                   allowed_repos_json=excluded.allowed_repos_json,
                   allowed_actions_json=excluded.allowed_actions_json,
                   status=excluded.status,
                   last_heartbeat_at=excluded.last_heartbeat_at",
                params![
                    input.id,
                    input.name,
                    input.endpoint,
                    input.os_family,
                    repos_json,
                    actions_json,
                    status_str,
                    ts
                ],
            )?;
            let dto = read_mesh_node(db, &input.id)?;
            Store::append_change(
                db,
                ChangeTopic::MeshNode,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn list_mesh_nodes(&self) -> Result<Vec<MeshNodeDto>> {
        self.read(|db| {
            let mut stmt = db.prepare(
                "SELECT id, name, endpoint, os_family, allowed_repos_json, allowed_actions_json,
                        status, last_heartbeat_at, created_at
                 FROM mesh_nodes ORDER BY rowid DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                let status: String = r.get(6)?;
                Ok(MeshNodeDto {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    endpoint: r.get(2)?,
                    os_family: r.get(3)?,
                    allowed_repos: parse_json(r.get(4)?, 4)?,
                    allowed_actions: parse_json(r.get(5)?, 5)?,
                    status: decode_enum(&status)?,
                    last_heartbeat_at: r.get(7)?,
                    created_at: r.get(8)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Error::from)
        })
    }

    pub fn get_mesh_node(&self, id: &str) -> Result<MeshNodeDto> {
        let id = id.to_owned();
        self.read(move |db| read_mesh_node(db, &id))
    }

    pub fn update_mesh_node_status(&self, id: &str, status: MeshNodeStatus) -> Result<MeshNodeDto> {
        let id = id.to_owned();
        let status_str = enum_value(status)?;
        self.write(move |db| {
            let existing = read_mesh_node(db, &id)?;
            if existing.status == MeshNodeStatus::Revoked {
                return Err(Error::ConflictError);
            }
            let ts = now(db)?;
            if status == MeshNodeStatus::Online {
                db.execute(
                    "UPDATE mesh_nodes SET status=?1, last_heartbeat_at=?2 WHERE id=?3 AND status!='revoked'",
                    params![status_str, ts, id],
                )?;
            } else {
                db.execute(
                    "UPDATE mesh_nodes SET status=?1 WHERE id=?2 AND status!='revoked'",
                    params![status_str, id],
                )?;
            }
            let dto = read_mesh_node(db, &id)?;
            Store::append_change(
                db,
                ChangeTopic::MeshNode,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn revoke_mesh_node(&self, id: &str) -> Result<MeshNodeDto> {
        let id = id.to_owned();
        self.write(move |db| {
            let changed = db.execute(
                "UPDATE mesh_nodes SET status='revoked', allowed_repos_json='[]', allowed_actions_json='[]' WHERE id=?1",
                [&id],
            )?;
            if changed == 0 {
                return Err(Error::NotFound);
            }
            let dto = read_mesh_node(db, &id)?;
            Store::append_change(
                db,
                ChangeTopic::MeshNode,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn upsert_notification_channel(
        &self,
        input: NewNotificationChannel,
    ) -> Result<NotificationChannelDto> {
        if input.label.trim().is_empty()
            || input.endpoint.trim().is_empty()
            || input.topic.trim().is_empty()
        {
            return Err(Error::Invalid(
                "channel label, endpoint, and topic are required",
            ));
        }
        let id = input.id.unwrap_or_else(|| new_id('n'));
        let kind_str = enum_value(input.kind)?;
        let enabled = i64::from(input.enabled);
        self.write(move |db| {
            let ts = now(db)?;
            db.execute(
                "INSERT INTO notification_channels (id, kind, label, endpoint, topic, enabled, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                   kind=excluded.kind,
                   label=excluded.label,
                   endpoint=excluded.endpoint,
                   topic=excluded.topic,
                   enabled=excluded.enabled",
                params![id, kind_str, input.label, input.endpoint, input.topic, enabled, ts],
            )?;
            let dto = read_channel(db, &id)?;
            Store::append_change(
                db,
                ChangeTopic::Notification,
                &dto.id,
                &serde_json::to_value(&dto)?,
            )?;
            Ok(dto)
        })
    }

    pub fn list_notification_channels(&self) -> Result<Vec<NotificationChannelDto>> {
        self.read(|db| {
            let mut stmt = db.prepare(
                "SELECT id, kind, label, endpoint, topic, enabled, created_at
                 FROM notification_channels ORDER BY rowid DESC",
            )?;
            let rows = stmt.query_map([], |r| {
                let kind: String = r.get(1)?;
                Ok(NotificationChannelDto {
                    id: r.get(0)?,
                    kind: decode_enum(&kind)?,
                    label: r.get(2)?,
                    endpoint: r.get(3)?,
                    topic: r.get(4)?,
                    enabled: r.get::<_, i64>(5)? != 0,
                    created_at: r.get(6)?,
                })
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
                .map_err(Error::from)
        })
    }

    pub fn delete_notification_channel(&self, id: &str) -> Result<()> {
        let id = id.to_owned();
        self.write(move |db| {
            let deleted = db.execute("DELETE FROM notification_channels WHERE id=?1", [&id])?;
            if deleted == 0 {
                return Err(Error::NotFound);
            }
            Store::append_change(
                db,
                ChangeTopic::Notification,
                &id,
                &serde_json::json!({"id": id, "deleted": true}),
            )?;
            Ok(())
        })
    }
}
