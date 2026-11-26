#!/bin/sh
# Scan what a push would publish for private details. Exit 1 on any finding.
# Usage: scripts/privacy-scan.sh [base]   (default base: origin/main, i.e. what the push would add)
set -eu
base="${1:-}"
if [ -z "$base" ]; then
  base=origin/main
fi
fail=0
pattern='/Users/[A-Za-z]|/home/[a-z]|C:\\Users\\|BEGIN [A-Z ]*PRIVATE KEY|sk-[A-Za-z0-9_-]{20,}|ghp_[A-Za-z0-9]{20,}|github_pat_|AKIA[0-9A-Z]{16}|xox[abp]-'
# 1. Added text lines in the commits to be pushed (every commit, not only the net diff).
if git log -p --no-color "$base"..HEAD -- . ':!scripts/privacy-scan.sh' | grep -E '^\+' | grep -vE '^\+\+\+ ' | grep -nE "$pattern"; then
  echo "privacy-scan: private path or secret in added lines (above)"; fail=1
fi
# 2. Email addresses other than placeholders and GitHub noreply.
if git log -p --no-color "$base"..HEAD -- . ':!scripts/privacy-scan.sh' | grep -E '^\+' | grep -oE '[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}' \
  | grep -vE '@(users\.noreply\.github\.com|example\.(com|invalid|org)|anthropic\.com)$' | sort -u | grep .; then
  echo "privacy-scan: email address in added lines (above)"; fail=1
fi
# 3. Files that must never be committed, and large files.
for f in $(git diff --name-only --diff-filter=AM "$base"..HEAD); do
  case "$f" in
    .handcheck/*|.impeccable/critique/*|.state-*|*.sqlite*|node_modules/*|.env*) echo "privacy-scan: must not be committed: $f"; fail=1 ;;
  esac
  if [ -f "$f" ] && [ "$(wc -c < "$f")" -gt 400000 ]; then echo "privacy-scan: file over 400 KB: $f"; fail=1; fi
done
# 4. Commit authors must use a noreply address.
if git log --format='%ae%n%ce' "$base"..HEAD | sort -u | grep -vE '@users\.noreply\.github\.com$|^noreply@anthropic\.com$' | grep .; then
  echo "privacy-scan: commit author/committer email is not a noreply address (above)"; fail=1
fi
# 5. Commit author/committer names must be the project account (web-UI commits use the profile name).
if git log --format='%an%n%cn' "$base"..HEAD | sort -u | grep -vxE 'DustNonStar|GitHub' | grep .; then
  echo "privacy-scan: commit author/committer name is not the project account (above)"; fail=1
fi
[ "$fail" -eq 0 ] && echo "privacy-scan: clean ($base..HEAD)"
exit "$fail"
