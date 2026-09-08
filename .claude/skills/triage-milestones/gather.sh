#!/usr/bin/env sh
# Prints the open milestones, every open issue grouped by milestone, and the
# issues closed in the current milestone. Read-only; the judgement is the
# lead's, not this script's.
set -eu
repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)

echo "## Milestones"
gh api "repos/$repo/milestones?state=open" \
  --jq '.[] | "\(.number)\t\(.title)\n    \(.description)"'

echo
echo "## Open issues"
gh issue list --state open --limit 200 \
  --json number,title,milestone,labels,body \
  --jq 'sort_by(.milestone.title // "~") | .[]
    | "#\(.number) [\(.milestone.title // "no milestone")] \(.title) [\(.labels|map(.name)|join(","))]\n    \(.body|gsub("\n";" ")[:240])"'

echo
echo "## Closed this cycle"
gh issue list --state closed --limit 50 --search "closed:>=$(git log --no-show-signature -1 --format=%cs "$(git describe --tags --abbrev=0)")" \
  --json number,title,milestone \
  --jq '.[] | "#\(.number) [\(.milestone.title // "-")] \(.title)"'
