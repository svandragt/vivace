---
name: triage-milestones
description: Re-triage the current GitHub milestone and shape the next one. Run after cutting a release, or when the user says "triage the milestone", "triage milestones", "/triage-milestones".
---

# Triage milestones

Two milestones at a time: the current one gets pruned to its theme, the
next one gets created and seeded. Judgement stays with the lead; the
script only gathers.

## Steps

1. Gather. Run `.claude/skills/triage-milestones/gather.sh` and read all of
   it. Also list parked items for this repo (`park list --remote <origin>`)
   and check `park` candidates that deserve an issue.
2. Judge every open issue in the current milestone against the milestone
   description, in one sentence each:
   - **Keep** if it serves the theme. Order by value and size, smallest
     unblocker first. Name the role that should do it (coder-low,
     coder-high, lead).
   - **Move** to the next milestone if it is a different theme, research,
     or a large orthogonal item (Windows, a rewrite).
   - **Back to backlog** if nothing planned needs it.
   Also check unmilestoned open issues and parked items for anything that
   belongs in either milestone.
3. Propose the next milestone: a title in the existing style
   (`0.N short theme`) and a one-sentence description that says what
   "done" means. List what seeds it.
4. Report keep / move / next as three short lists, then stop. Milestone
   and issue writes are outward-facing: wait for the user's go.
5. On go: create the milestone, move issues, file any parked item that
   became an issue (then `park done` it), and offer to start the first
   keep item.

## Rules

- Never write park IDs, client names or agent narration into issues; see
  the repo's anonymisation rule. Issues cite `file:line` and a done-when.
- The milestone counter GitHub shows can lag; count the issues yourself.
- One bare `#N` per issue is fine in the report, with its title on first
  mention.
