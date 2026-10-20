# Contributing to StarCLX

Thanks for your interest in improving StarCLX! Issues and pull requests may be written in
English or German.

## Reporting bugs and suggesting features

Please use the [issue forms](https://github.com/crazmoe/StarCLX/issues/new/choose). For bugs,
include the StarCLX version (shown at the bottom of the Settings), the installation method, your
operating system and the STARFACE version. Remove passwords, tokens and other personal data
from logs and screenshots.

Rough ideas and questions are welcome in
[Discussions](https://github.com/crazmoe/StarCLX/discussions), where others can comment and
vote on them; a concrete feature request can then become an issue.

Security problems are reported privately, see [SECURITY.md](SECURITY.md).

## Development setup

Requirements: Rust (stable), Node 22, and on Debian/Ubuntu
`libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libdbus-1-dev`
(full list in the "Systempakete" step of [`.github/workflows/ci.yml`](workflows/ci.yml)).

```sh
git clone --recurse-submodules https://github.com/crazmoe/StarCLX.git
cd StarCLX
cargo test --workspace
cd apps/desktop && npm install && npm run tauri dev
```

The layout of the repository is described under "Structure" in the [README](../README.md).

## Before opening a pull request

CI runs the following checks; please run them locally first:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop && npm run check && npm run i18n:check
```

- UI strings go through `t()`, with the German text as the key. Add the translations in
  `apps/desktop/src/lib/i18n/`; `npm run i18n:check` reports missing entries.
- Keep a pull request focused on one change and describe what a user will notice.
- User-visible changes get a line in [`CHANGELOG.md`](../CHANGELOG.md).
- Do not commit credentials, server addresses or recordings from real systems.

## License

By contributing, you agree that your contributions are licensed under the
[MIT License](../LICENSE).

## Code of Conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). By taking part you agree
to uphold it.
