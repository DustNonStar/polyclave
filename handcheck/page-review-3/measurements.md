# Page review 3 hand-check measurements

## Screenshots
- `item7-desktop-locked.jpg`: 114.7 KB
- `item7-phone-locked-needs.jpg`: 53.6 KB
- `item7-phone-details-open.jpg`: 58.8 KB
- `item4-desktop-open-run.jpg`: 127.7 KB
- `item4-phone-open-run.jpg`: 60.7 KB
- `item5-desktop-unlocked-review.jpg`: 120.0 KB
- `item2-phone-what-changed.jpg`: 54.6 KB
- `item6-desktop-confirm-arming.jpg`: 122.6 KB
- `item6-phone-confirm.jpg`: 60.2 KB
- `item2-desktop-accepted-commit.jpg`: 121.6 KB
- `item3-desktop-send-failed.jpg`: 124.5 KB
- `item3-phone-send-failed.jpg`: 32.8 KB
- `item3-desktop-send-recovered.jpg`: 125.0 KB

## Checks
```json
[
  {
    "label": "item7-desktop-locked",
    "detailsOpen": true,
    "cmds": [
      "ps -g 83715",
      "git -C /private/tmp/polyclave-hc/page-review-3/state/worktrees/raaea020a status"
    ],
    "selectedSession": "api-server",
    "composerPlaceholder": "Ask api-server — messages sent during a turn wait their turn"
  },
  {
    "label": "item7-desktop-locked",
    "docOverflow": 0,
    "badCount": 3,
    "bad": [
      {
        "tag": "SPAN",
        "cls": "avatar",
        "id": "",
        "sw": 35,
        "cw": 32,
        "ox": "visible"
      },
      {
        "tag": "SPAN",
        "cls": "avatar",
        "id": "",
        "sw": 35,
        "cw": 32,
        "ox": "visible"
      },
      {
        "tag": "SPAN",
        "cls": "avatar",
        "id": "",
        "sw": 35,
        "cw": 32,
        "ox": "visible"
      }
    ]
  },
  {
    "label": "item7-phone-locked",
    "activeTab": "Needs you 1",
    "detailsOpen": false,
    "shortPath": "…/worktrees/raaea020a",
    "fullPathTitle": "/private/tmp/polyclave-hc/page-review-3/state/worktrees/raaea020a",
    "hintText": "Can't check from here? Keep it locked; nothing runs until you remove the lock.",
    "removeLockBottom": 588.359375,
    "viewportHeight": 812
  },
  {
    "label": "item7-phone-locked-needs",
    "docOverflow": 0,
    "badCount": 0,
    "bad": []
  },
  {
    "label": "item7-phone-details-open",
    "docOverflow": 0,
    "badCount": 0,
    "bad": []
  },
  {
    "label": "item4-desktop-open-run",
    "selectedTab": "web",
    "selectedSidebar": "web",
    "composerPlaceholder": "Ask web — messages sent during a turn wait their turn",
    "crumb": "ViewingWEweb·raaea020a"
  },
  {
    "label": "item4-phone-open-run",
    "activePhoneTab": "side",
    "phoneHeadSession": "web",
    "composerPlaceholder": "Ask web — messages sent during a turn wait their turn"
  },
  {
    "label": "item4-phone-open-run",
    "docOverflow": 0,
    "badCount": 0,
    "bad": []
  },
  {
    "label": "item5-and-item2-desktop",
    "questionText": "Review 03be558: accept it, mark it failed, or allow a retry.",
    "hostNoteText": "Host note: An agent process may still be writing.",
    "commitHead": "What changed",
    "commitSubj": "feat: add nested widget, binary asset and styles (raaea020a)",
    "commitFiles": [
      {
        "path": "README.md",
        "stat": "+1 −0"
      },
      {
        "path": "assets/icon.bin",
        "stat": "binary"
      },
      {
        "path": "docs/notes.md",
        "stat": "+2 −0"
      },
      {
        "path": "src/components/deep/nested/directory/very-long-component-name.ts",
        "stat": "+2 −0"
      },
      {
        "path": "src/styles/theme.css",
        "stat": "+1 −0"
      }
    ],
    "commitMore": "and 2 more files"
  },
  {
    "label": "item5-desktop-unlocked-review",
    "docOverflow": 0,
    "badCount": 3,
    "bad": [
      {
        "tag": "SPAN",
        "cls": "avatar",
        "id": "",
        "sw": 35,
        "cw": 32,
        "ox": "visible"
      },
      {
        "tag": "SPAN",
        "cls": "avatar",
        "id": "",
        "sw": 35,
        "cw": 32,
        "ox": "visible"
      },
      {
        "tag": "SPAN",
        "cls": "avatar",
        "id": "",
        "sw": 35,
        "cw": 32,
        "ox": "visible"
      }
    ]
  },
  {
    "label": "item2-phone-what-changed",
    "docOverflow": 0,
    "badCount": 0,
    "bad": []
  },
  {
    "label": "item6-double-tap",
    "resolvesBeforeDoubleTap": 0,
    "resolvesRightAfterDoubleTap": 0,
    "backLeft": 1107,
    "confirmLeft": 1178.765625,
    "confirmOpacity": "0.45",
    "confirmAriaDisabled": "true",
    "fineText": "Accept applies to 03be558 only (03be558e7d15b599228bbc991d9440377e4ed438). If the branch moves first, you will be asked to review the new tip."
  },
  {
    "label": "item6-phone-confirm-armed",
    "confirmOpacity": "1",
    "confirmAriaDisabled": "false",
    "hasArmingClass": false
  },
  {
    "label": "item6-phone-confirm",
    "docOverflow": 0,
    "badCount": 0,
    "bad": []
  },
  {
    "label": "item2-accepted-commit",
    "resolvesAfterConfirm": 1
  },
  {
    "label": "item3-desktop-send-failed",
    "composerValue": "Keep this draft when host is down",
    "errorText": "Not sent: Failed to fetch. Your message is still here.",
    "sendDisabled": false
  },
  {
    "label": "item3-phone-send-failed",
    "docOverflow": 0,
    "badCount": 1,
    "bad": [
      {
        "tag": "DIV",
        "cls": "act",
        "id": "",
        "sw": 368,
        "cw": 343,
        "ox": "visible"
      }
    ]
  },
  {
    "label": "item3-desktop-send-recovered",
    "composerValue": "",
    "errorHidden": true,
    "errorText": ""
  },
  {
    "label": "resources",
    "count": 115,
    "external": []
  }
]
```
