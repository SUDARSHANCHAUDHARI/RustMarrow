# Contributing

Thanks for considering a contribution.

## Local Setup

```bash
git clone https://github.com/SUDARSHANCHAUDHARI/RustMarrow.git
cd RustMarrow
cargo test
```

## Quality Checks

Run these before opening a pull request:

```bash
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build --release
```

## Pull Request Guidelines

- Keep changes focused and easy to review.
- Add or update tests for behavior changes.
- Update the README or examples when CLI behavior changes.
- Do not commit secrets, `.env` files, tokens, private keys, signing material, or local machine config.
- Explain user-visible behavior changes in the pull request description.

## Release Notes

For release-facing changes, add a short entry to `CHANGELOG.md` under an unreleased section or the next planned version.
