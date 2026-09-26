# Linear

Minutes uses Linear's GraphQL API (`https://api.linear.app/graphql`) with a personal API key. The key is sent as `Authorization: <key>` and stored in the OS credential store. The auth type is abstracted (`LinearAuth`), so OAuth can be added later.

## Setup

1. In Linear: **Settings → Security & access → Personal API keys**. Create a key.
2. In Minutes: **Settings → Linear**. Paste the key, which is verified with a `viewer` query.
3. Optional:
   - set a default team and project;
   - link people to Linear users. This is a deterministic mapping from each Minutes person to a Linear user ID.

## Review before anything is created

Issues are never created from raw model output. **Create selected Linear issues** opens a review sheet. It lists only the action items you ticked, each with its own editable fields:

- **Title**
- **Assignee:** pre-filled only from a person-to-Linear-user mapping.
- **Team:** your default, or the only team.
- **Project:** filtered to the team.
- **Priority**
- **Due date:** resolved from what was said, such as "Friday" relative to the meeting date.
- **Description:**

```
Created from Design Weekly — 26 Sep 2026

## Context
…

## Evidence
Tom · 28:43
> I'll take the API changes and have them ready Friday.

Meeting notes: <Notion URL if sent>
```

Every ID is checked against the live Linear directory, and the model never supplies one. After an issue is created, its ID, identifier (e.g. ENG-42) and URL are stored on the action item and shown as a link.

## No duplicates on retry

- **Before the first request,** each action item gets a client-generated UUID v4. It is saved (`linear_idempotency_key`) and sent as `IssueCreateInput.id`.
- **If the outcome is unknown** (for example a network failure after sending), the item is marked `unknown`. A retry first looks up `issue(id: key)`, and re-sending the same ID can't create a second issue.
- **Partial failures are recorded per item,** with the exact error, in `action_items` and `integration_operations`. Items that succeeded are never re-sent.

Tested against a local fake Linear server: a lost response followed by a retry results in exactly one create.

## Rate limits

When Linear rate-limits a request it returns HTTP 400 with `RATELIMITED`. Minutes reports this plainly ("Linear is busy…"), and the item can be retried safely.
