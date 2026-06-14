#!/usr/bin/env bash
set -euo pipefail

repository="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
binary="${SAMESESSION_BIN:-$repository/target/debug/samesession}"
root="$(mktemp -d "${TMPDIR:-/tmp}/samesession-e2e.XXXXXX")"
trap 'rm -rf "$root"' EXIT

remote="$root/remote.git"
source="$root/source"
destination="$root/destination"
worktree="$root/resumed-worktree"
source_codex="$root/source-codex"
destination_codex="$root/destination-codex"
identity="$root/identity.age"
session="019f-e2e-dirty-migration"

git init -q --bare "$remote"
git clone -q "$remote" "$source"
git -C "$source" config user.name SameSessionTest
git -C "$source" config user.email test@samesession.local
printf 'base\n' > "$source/file.txt"
printf 'remove me\n' > "$source/deleted.txt"
git -C "$source" add file.txt deleted.txt
git -C "$source" commit -qm base
git -C "$source" push -q origin HEAD:main
git --git-dir="$remote" symbolic-ref HEAD refs/heads/main
git clone -q "$remote" "$destination"

printf 'dirty tracked\n' > "$source/file.txt"
rm "$source/deleted.txt"
printf '\000\001\002\377' > "$source/untracked.bin"
source_status="$(git -C "$source" status --porcelain=v1 --untracked-files=all)"
destination_head="$(git -C "$destination" rev-parse HEAD)"

mkdir -p "$source_codex/sessions/2026/06/14" "$destination_codex"
printf '{"timestamp":"2026-06-14T12:00:00Z","type":"session_meta","payload":{"id":"%s","cwd":"%s","cli_version":"0.137.0"}}\n{"type":"response_item","payload":{"text":"continue this work"}}\n' \
  "$session" "$source" > "$source_codex/sessions/2026/06/14/rollout-$session.jsonl"

recipient="$(CODEX_HOME="$source_codex" "$binary" device init --identity "$identity")"
move="$(
  CODEX_HOME="$source_codex" "$binary" move "$session" \
    --provider codex \
    --recipient "$recipient" \
    --repository "$source" \
    --identity "$identity" \
    --push origin \
    --json
)"
portable="$(jq -r '.checkpoint.public.portable_session_id' <<<"$move")"

CODEX_HOME="$destination_codex" "$binary" resume "$portable" \
  --provider codex \
  --repository "$destination" \
  --identity "$identity" \
  --remote origin \
  --into "$worktree" \
  --no-launch \
  --force-native \
  --json >/dev/null

test "$(git -C "$source" status --porcelain=v1 --untracked-files=all)" = "$source_status"
test "$(git -C "$destination" rev-parse HEAD)" = "$destination_head"
test "$(cat "$worktree/file.txt")" = "dirty tracked"
test ! -e "$worktree/deleted.txt"
cmp "$source/untracked.bin" "$worktree/untracked.bin"
cmp \
  "$source_codex/sessions/2026/06/14/rollout-$session.jsonl" \
  "$destination_codex/sessions/2026/06/14/rollout-$session.jsonl"
test -z "$(git -C "$source" for-each-ref refs/samesession/capture)"

if CODEX_HOME="$destination_codex" "$binary" resume "$portable" \
  --provider codex \
  --repository "$destination" \
  --identity "$identity" \
  --remote origin \
  --into "$worktree" \
  --no-launch \
  --force-native >/dev/null 2>&1
then
  echo "second resume unexpectedly succeeded" >&2
  exit 1
fi

lease="$("$binary" lease status "$portable" --repository "$destination" --json)"
jq -e '.lease.released == true' <<<"$lease" >/dev/null
