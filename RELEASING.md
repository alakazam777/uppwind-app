# Releasing the companion

A release is `uppwind-companion.exe` and its SHA-256, nothing else. Never publish the `.pdb`, the `.d` files or the `target/` folder: they hold the build machine's paths.

## The workflows (GitHub Actions)

- `.github/workflows/ci.yml`, on every push and pull request: the companion's tests on Windows (`cargo test --release --locked`, the registry test included: it writes `HKCU\Software\UppwindLinkTest` on the runner and deletes it), the Python tests, `third_party_notices.py --check`; the companion's tests on macOS (arm64, build and tests only, no signing); `cargo audit` and `cargo deny` (rules in `deny.toml`); `gitleaks` on the files and on the git history.
- `.github/workflows/release.yml`, on a tag `v*` (it must match `version` in `companion/Cargo.toml`): steps 3 to 5 and 7 below on a `windows-2025` runner, then a GitHub release with `uppwind-companion.exe` and `SHA256SUMS` only, and an artifact attestation. To release: bump the version in `companion/Cargo.toml` (and `Cargo.lock`), wait for `ci` to be green on `main`, then `git tag v<version>` and `git push origin v<version>`. macOS is not released yet (no notarization).

Both pin every action by its commit SHA and every tool by its version (gitleaks by its SHA-256 too) and use the same Rust version (`RUST_VERSION`); update them together.

## Before every release

1. **Dependencies** (online): `cargo audit --file companion/Cargo.lock` and `cargo deny --manifest-path companion/Cargo.toml check --config deny.toml advisories bans licenses sources` must pass (the `ci` workflow runs both). Then `python -I dev/third_party_notices.py --check`: after any dependency change, rerun it without `--check` and commit `THIRD-PARTY-NOTICES.txt`.
2. **Secrets and personal data** in the sources (and in the git history of this repository): `gitleaks detect` must find nothing. Look at every new file before the commit that adds it.
3. **Tests**: the commands of *Build from source* in the README, all green.
4. **Build from a clean checkout of the release tag**, ideally in CI (a GitHub Actions Windows runner), never from a working copy with stray files:

   ```
   cargo --config "target.x86_64-pc-windows-msvc.rustflags=['--remap-path-prefix=<CARGO_HOME>=/cargo', '--remap-path-prefix=<checkout folder>=/src']" build --release --locked --manifest-path companion/Cargo.toml
   ```

   Run it from the repository folder, so that cargo reads `.cargo/config.toml` (the static C runtime, `/DEPENDENTLOADFLAG:0x800`, `/Brepro`); `--config` adds the path remapping to those flags. Don't set `RUSTFLAGS`: it would replace them all. Write down the Rust version (`rustc -V`) and the Visual Studio build tools and Windows SDK versions with the release.
5. **Check the program**: `python -I dev/release_check.py companion/target/release/uppwind-companion.exe` must print `ok` (no user, cargo or drive paths; DLLs from System32 only; no separate C runtime; a reproducible build). Build twice from clean checkouts at the **same folder path** (each into a fresh `target`): the two SHA-256 must be equal. The folder path is part of the build: it sits in the `--remap-path-prefix` flags, which cargo hashes into every crate's symbol names, so the same sources built from another folder give another (equally valid) exe. Write the folder path down with the release (in GitHub Actions it is always `D:\a\uppwind-app\uppwind-app`), so anyone can rebuild it byte for byte.
6. **Sign it** when a certificate is available (SignPath Foundation is free for open-source projects; Azure Trusted Signing; an OV certificate), then check the signature.
7. **Publish** the exe and a `SHA256SUMS` file on the GitHub release; if the build runs in GitHub Actions, add an artifact attestation (`actions/attest-build-provenance`) so that `gh attestation verify` works.

## The repository itself

- Commit with the GitHub *noreply* address only, set in this repository (`git config user.email <id>+<login>@users.noreply.github.com`), never globally. Before a push, `git log --format="%an <%ae> | %cn <%ce>"` must show only that address.
- On GitHub: *Keep my email addresses private* and *Block command line pushes that expose my email*, two-factor authentication, secret scanning with push protection, Dependabot alerts, private vulnerability reporting (SECURITY.md relies on it).
- CI workflows (`.github/workflows/`, see above): actions pinned by commit SHA, `permissions: contents: read` by default, `contents: write`, `id-token: write` and `attestations: write` only in the release job.
- Outside contributions: decide before the first pull request how they are licensed (a DCO, "inbound = outbound", is the simplest), or relicensing later needs every contributor's consent.
