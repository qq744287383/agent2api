# Triage Labels

The skills speak in terms of five canonical triage roles. This file maps those roles to the actual label strings used in this repo's issue tracker.

| Label in mattpocock/skills | Label in our tracker | Meaning                                  |
| -------------------------- | -------------------- | ---------------------------------------- |
| `needs-triage`             | `needs-triage`       | Maintainer needs to evaluate this issue  |
| `needs-info`               | `needs-info`         | Waiting on reporter for more information |
| `ready-for-agent`          | `ready-for-agent`    | Fully specified, ready for an AFK agent  |
| `ready-for-human`          | `ready-for-human`    | Requires human implementation            |
| `wontfix`                  | `wontfix`            | Will not be actioned                     |

When a skill mentions a role (e.g. "apply the AFK-ready triage label"), use the corresponding label string from this table.

## Wayfinder labels

`/wayfinder` uses a second, separate vocabulary. A map is one issue labelled `wayfinder:map`; each of its child tickets carries exactly one `wayfinder:<type>` label:

| Label | Meaning |
| --- | --- |
| `wayfinder:map` | The map itself: destination, notes, decisions-so-far, fog |
| `wayfinder:research` | Ticket type: research |
| `wayfinder:prototype` | Ticket type: prototype |
| `wayfinder:grilling` | Ticket type: grilling |
| `wayfinder:task` | Ticket type: task |

All ten labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`, `wayfinder:map`, `wayfinder:research`, `wayfinder:prototype`, `wayfinder:grilling`, `wayfinder:task`) already exist in this repo's label list. Apply only the ones a skill actually calls for — no other label is mandatory.
