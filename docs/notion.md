# Notion

Minutes calls the Notion API directly, using Notion-Version `2026-03-11` and the data-source model. It doesn't use Zapier, Make or Notion AI.

## Setup

1. In Notion: **Settings → Connections → Develop or manage integrations**. Create an internal integration and copy its secret.
2. Share your meetings database with it: open the database, then **••• → Connections → add the integration**.
3. In Minutes: **Settings → Notion**. Paste the secret, which is verified against `/v1/users/me` and stored in the OS credential store.
4. Pick the database. Minutes searches data sources shared with the integration and checks that the database has a title property.
5. Choose the upload mode:
   - *Summary + actions only*;
   - *Summary + transcript*;
   - *Ask each time*.
6. Optional: link people to Notion users so action items `@mention` them.

## What is sent

Only when you press **Send to Notion** on a meeting. The page title is the meeting title, and the body is:

```
26 September 2026 · 47 minutes
## Summary          (bullets)
## Decisions        (bullets)
## Action items     (to-dos: @Tom — Update onboarding API (due 2026-10-02))
## Open questions   (bullets)
## Transcript       (optional; "### Walter · 00:02:13" + paragraphs)
```

The page ID and URL are saved on the meeting, with an **Open in Notion** link.

## Limits and reliability

- Text is split to Notion's 2000-character limit.
- Pages are created with the first 100 blocks and extended in batches of 100 using `PATCH /v1/blocks/{id}/children` with `position: end`.
- 429/529 responses honour `Retry-After`, with up to 4 attempts.
- **Failures keep the local notes intact** and can be retried.
- **Re-sending** creates a fresh page first. Only after that succeeds is the previous page moved to Notion's trash, so notes are never lost.
- Every attempt is recorded in `integration_operations`.
