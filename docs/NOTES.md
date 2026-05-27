# Notes

## Why This Exists

Personal AI tools become more useful when they remember durable context, but that memory should not require a hosted service or careless credential handling. RustMarrow explores a local-first version of that idea.

## Known Limits

- External pullers require correct local credentials.
- Provider APIs can change independently from this repo.
- There are no integration tests yet for every source puller.

## Maintenance Notes

- Never commit real tokens, refresh tokens, emails, or personal exports.
- Keep examples placeholder-only.
- Treat schema changes carefully so local data is not lost.
