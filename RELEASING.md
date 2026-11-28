# Releasing the companion

A release is `uppwind-companion.exe` (and, once switched on, `uppwind-companion-macos.dmg`), their SHA-256 (`SHA256SUMS`) and the signature of that file (`SHA256SUMS.minisig`, made offline with the update key: step 8), nothing else. The Uppwind Link plugins are **inside** the companion (it installs them itself), built from the same tag with the same flags, so they are not a separate download; the .dmg also holds the signed `Uppwind Link.vst3` and `Uppwind Link.component` (the Audio Units, for Logic Pro). Never publish a `.pdb`, the `.d` files or a `target/` folder: they hold the build machine's paths.

## The workflows (GitHub Actions)

- `.github/workflows/ci.yml`, on every push and pull request: on Windows, the plugins' tests, the plugins built and bundled, the companion's tests with that bundle inside (`cargo test --release --locked`, the registry test included: it writes `HKCU\Software\UppwindLinkTest` on the runner and deletes it), the companion built and checked to install exactly those plugin bytes (into a folder of the runner, `--plugins-dir`) and remove them, the Python tests, `third_party_notices.py --check`; the same on macOS (arm64, the plugins built with their Audio Units, the VST3 and Audio Unit bundles signed ad hoc, the installed copies checked with `codesign --verify`; no Developer ID); `cargo audit` and `cargo deny` for the companion and for the plugins (rules in `deny.toml`); `gitleaks` on the files and on the git history.
- `.github/workflows/release.yml`, on a tag `v*` (it must match `version` in `companion/Cargo.toml`): `cargo audit` and `cargo deny` again (step 1, the same commands and versions as `ci.yml`; a failure stops the release; the macOS build waits for them and for the Windows build, so that no signing is approved for a release that will not ship), steps 3 to 5 and 7 below on a `windows-2025` runner, then a **draft** GitHub release with `uppwind-companion.exe` and `SHA256SUMS` only, and an artifact attestation. It refuses to build a companion that knows no update key (`--update-keys`). The draft is invisible to the companions (`releases/latest` skips drafts); it becomes the release only once you have signed it (step 8). To release: bump the version in `companion/Cargo.toml` (and `Cargo.lock`), wait for `ci` to be green on `main`, then `git tag v<version>` and `git push origin v<version>`, then step 8.
- `.github/workflows/macos-build.yml` (called by the two below), on `macos-26` runners (pinned: the five jobs on one image, the one `macos-latest` gave on 24 Nov), in five jobs (24 Nov 2026, audit M8: **no job that runs `cargo` or what it built ever holds a secret**, and the two jobs that hold the certificate run only `codesign`, `hdiutil`, `notarytool` and `stapler`):
  1. `build-plugins` (no secret): the tests (plugins, companion), `third_party_notices.py --check`, then `UWL_STAGE=plugins` `dev/build-macos.sh` with `UWL_RELEASE=1` (`--locked`, the runner's paths remapped): the plugins, universal (arm64 + x86_64), bundled and **not signed**; `release_check.py` on them.
  2. `sign-plugins` (environment `release`): a keychain made for the job with the *Developer ID Application* certificate (key not exportable, deleted at the job's end whatever happens); `Uppwind Link.vst3` and `Uppwind Link.component` signed (hardened runtime, Apple's timestamp) and checked (`Authority=Developer ID Application`, `Timestamp`, `runtime`). No checkout.
  3. `build-companion` (no secret): `UWL_STAGE=companion`, the universal companion and `uppwind-link-host`, with the bundles **signed in job 2** inside (`UWL_PLUGIN_BUNDLE`, `UWL_PLUGIN_COMPONENT`), left unsigned; `release_check.py`; an ad hoc signed copy checked to carry the notices and to install exactly the signed bundles (`diff -r`).
  4. `sign-notarize` (environment `release`): the plugins from job 2's own artifact, the binaries from job 3's; the companion signed (hardened runtime, timestamp, identifier `com.uppwind.link.companion`, **no entitlements**), `uppwind-link-host` signed with `dev/host-entitlements.plist` (**`disable-library-validation` only**; the plist comes from the job's checkout, never from an artifact); `uppwind-companion-macos.dmg` with the companion, the helper, `Uppwind Link.vst3` and `Uppwind Link.component` (so that notarization covers them, and a user can drag them into `~/Library/Audio/Plug-Ins/VST3` and `~/Library/Audio/Plug-Ins/Components`), the README, the licences and both notices; the .dmg signed, notarized (`notarytool` with an App Store Connect API key, its log printed) and stapled. Its SHA-256 is the workflow's output `dmg-sha256`.
  5. `verify` (no secret): the .dmg checked like a user gets it (`stapler validate`, `spctl`, both signatures and the entitlements in the mounted .dmg, `--license`, the companion of the .dmg installing its bundles, `diff -r`, `codesign --verify --deep --strict`), and `auval` on its Audio Units.

  Between jobs the files travel as one tar (upload-artifact would drop the executable bits); the job that made it gives its SHA-256 as an output, the next one checks it (`shasum -c`) and the list of entries before extracting into an empty folder. The environment `release` asks for approval **twice per run** (jobs 2 and 4). What this does not prevent: a compromised dependency can still alter the plugin or the companion it helps build, and that code gets signed; it can no longer sign anything else, nor take the key.

  Secrets (in the environment `release`): `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD`, `APPLE_API_KEY_P8_BASE64` (the `AuthKey_<key id>.p8` file, base64), `APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`. `APPLE_ID`, `APPLE_TEAM_ID` and `APPLE_APP_PASSWORD` are no longer read: delete them, and revoke the app-specific password at account.apple.com, once a notarization with the API key has passed. The key: App Store Connect › *Users and Access* › *Integrations* › *App Store Connect API* › *Team Keys* › *Generate API Key* (an Admin account is needed; a **Team** key: Apple says an Individual key cannot use notarytool). Its access: the lowest one that notarytool accepts, *Developer* as far as we know (not confirmed in Apple's documentation: check it when creating the key). The page shows the key's *Key ID* after it is generated, and the team's *Issuer ID* on the same page (place to confirm); the `.p8` file can be downloaded **once only**. `base64 -i AuthKey_<key id>.p8 | pbcopy` gives the value of `APPLE_API_KEY_P8_BASE64`. A key can be revoked on the same page (*Edit*, *Revoke Key*), at once and for good.
- `.github/workflows/macos-preview.yml`, started by hand (`gh workflow run macos-preview.yml`): that .dmg as a workflow artifact only, no release.
- macOS in a release: only when the repository variable `MACOS_RELEASE` is `1` (`gh variable set MACOS_RELEASE --body 1`); then `release.yml` adds `uppwind-companion-macos.dmg` to the release and to `SHA256SUMS`, after checking its SHA-256 against the output `dmg-sha256` of `macos-build.yml`. Off by default: a tag ships Windows only.
- The environment `release` should accept only `main` and the tags `v*` (*Settings › Environments › release › Deployment branches and tags*: *Selected branches and tags*, add `main` and the tag rule `v*`), and *Prevent self-review* / no admin bypass if the plan allows it. With that, `macos-preview` runs on `main` only: try a branch's workflow changes before merging by allowing that branch for the time of the test.

Both pin every action by its commit SHA and every tool by its version (gitleaks by its SHA-256 too) and use the same Rust version (`RUST_VERSION`); update them together.

## Before every release

1. **Dependencies** (online): `cargo audit --deny warnings --file companion/Cargo.lock` (two advisories of Linux-only crates ignored, see `ci.yml`), `cargo audit --deny warnings --file plugin/Cargo.lock`, and `cargo deny --manifest-path <companion/Cargo.toml, then plugin/Cargo.toml> --config deny.toml check advisories bans licenses sources` must pass (the `ci` workflow runs them all). Then `cargo fetch --locked` for both manifests and `python -I dev/third_party_notices.py --check`: after any dependency change, rerun it without `--check` and commit `THIRD-PARTY-NOTICES.txt` and `plugin/THIRD-PARTY-NOTICES.txt` (each lists the Windows crates, then the macOS-only ones).
2. **Secrets and personal data** in the sources (and in the git history of this repository): `gitleaks detect` must find nothing. Its only exception is in `.gitleaks.toml` (the made-up test vectors of `fixtures/auth.json`, rule `generic-api-key` only); a made-up code in a test carries an inline `gitleaks:allow`. Never widen it. Look at every new file before the commit that adds it.
3. **Tests**: the commands of *Build from source* in the README, all green.
4. **Build from a clean checkout of the release tag**, ideally in CI (a GitHub Actions Windows runner), never from a working copy with stray files. The plugins first, with the same flags as the companion, then their bundle, then the companion with that bundle inside:

   ```
   set REMAP=target.x86_64-pc-windows-msvc.rustflags=['--remap-path-prefix=<CARGO_HOME>=/cargo', '--remap-path-prefix=<checkout folder>=/src']
   cargo --config "%REMAP%" build --release --locked --manifest-path plugin/Cargo.toml
   plugin\target\release\uwl-bundle.exe --out plugin\target\release\bundled
   for /f %t in ('git log -1 --format^=%ct') do set SOURCE_DATE_EPOCH=%t
   python -I dev/pe_timestamp.py "plugin/target/release/bundled/Uppwind Link.vst3/Contents/x86_64-win/Uppwind Link.vst3"
   set UWL_PLUGIN_BUNDLE=<checkout folder>\plugin\target\release\bundled\Uppwind Link.vst3
   cargo --config "%REMAP%" build --release --locked --manifest-path companion/Cargo.toml
   python -I dev/pe_timestamp.py companion/target/release/uppwind-companion.exe
   ```

   Run it from the repository folder, so that cargo reads `.cargo/config.toml` (the static C runtime, `/DEPENDENTLOADFLAG:0x800`, `/Brepro`: for the plugins' DLL too); `--config` adds the path remapping to those flags. Don't set `RUSTFLAGS`: it would replace them all. `UWL_PLUGIN_BUNDLE` must be an absolute path; `companion/build.rs` embeds every file of that bundle and stops on anything but the expected layout (the DLL alone on Windows). `dev/pe_timestamp.py` puts the commit date of the tag in the PE headers of the DLL (before the companion embeds it) and of the exe: `/Brepro` alone writes a hash there, which reads as a date anywhere (0.4.15 claimed 2091, a "timestamp in the future" for malware scanners); the date comes from the tag, so the build stays reproducible. Write down the Rust version (`rustc -V`) and the Visual Studio build tools and Windows SDK versions with the release.
5. **Check the plugins and the program**: `python -I dev/release_check.py "plugin/target/release/bundled/Uppwind Link.vst3/Contents/x86_64-win/Uppwind Link.vst3" companion/target/release/uppwind-companion.exe` must print `ok` (no user, cargo or drive paths, in the DLL nor in the exe that carries it; DLLs from System32 only; no separate C runtime; reproducible builds; a real build date in the PE headers). Then the program must install exactly that DLL: `uppwind-companion.exe --install-plugins --plugins-dir <an empty folder>` and compare the SHA-256 of the installed `Uppwind Link.vst3\Contents\x86_64-win\Uppwind Link.vst3` with the checked one (`--install-plugins --dry-run` also prints the SHA-256 of the files it carries). Build twice from clean checkouts at the **same folder path** (each into fresh `target` folders): the two SHA-256 must be equal, for the DLL and for the exe. The folder path is part of the build: it sits in the `--remap-path-prefix` flags, which cargo hashes into every crate's symbol names, so the same sources built from another folder give another (equally valid) exe. Write the folder path down with the release (in GitHub Actions it is always `D:\a\uppwind-app\uppwind-app`), so anyone can rebuild it byte for byte.

   macOS (`macos-build.yml` does it, in stages: `UWL_STAGE=plugins` builds the plugin bundles unsigned, a job without `cargo` signs them, `UWL_STAGE=companion` with `UWL_PLUGIN_BUNDLE` and `UWL_PLUGIN_COMPONENT` builds the companion around those signed bundles; by hand, in one go on a Mac that holds the certificate): `UWL_RELEASE=1 UWL_NO_ADHOC=1 UWL_PLUGIN_SIGN_IDENTITY=<Developer ID Application identity> UWL_SIGN_KEYCHAIN=<keychain> sh dev/build-macos.sh <folder>` builds and signs the plugins, then the companion that embeds them; `release_check.py` on `uppwind-companion`, on `Uppwind Link.vst3/Contents/MacOS/Uppwind Link` and on `Uppwind Link.component/Contents/MacOS/Uppwind Link` (the same binary; `UWL_AUV2=0` builds without the Audio Units). The plugin's install name is neutral (`plugin/plugins/build.rs`), so the dylib holds no build path either. A plugin bundle is signed before the companion embeds it: re-signing it afterwards would make the embedded copy differ from the published one.
6. **Sign it** when a certificate is available (SignPath Foundation is free for open-source projects; Azure Trusted Signing; an OV certificate), then check the signature.
7. **Publish** the exe and a `SHA256SUMS` file on a **draft** GitHub release; if the build runs in GitHub Actions, add an artifact attestation (`actions/attest-build-provenance`) so that `gh attestation verify` works. `release.yml` does both.
8. **Sign `SHA256SUMS` on your own computer, then publish the draft.** Companions from 0.4.16 on refuse a release without this signature. The update key never goes to GitHub, so this step is always done by hand, after the workflow. With *immutable releases* on, files can be added to a draft only: once published, nothing can be added or fixed, so **check before you publish**. Your signature is what the companions trust: whoever holds the GitHub account can push commits and tags, and start the workflow, so nothing GitHub says is proof on its own.

   **The tag comes from your own clone.** Create it there yourself, on the commit you mean to release, and write its hash down at that moment; never sign for a tag you only fetched. Better, sign the tag (`git tag -s v0.4.16`, with a GPG or SSH signing key), so that `git verify-tag v0.4.16` proves it is yours:

   ```
   git tag -s v0.4.16 -m "uppwind-companion v0.4.16"
   git rev-parse "v0.4.16^{commit}"
   git push origin v0.4.16
   ```

   **Read what changed in the update code** since the release before it, in your clone, before you sign anything:

   ```
   git log -p v0.4.15..v0.4.16 -- tools/uppwind-link/companion/src/update.rs tools/uppwind-link/companion/src/wizard/mod.rs
   ```

   A change you did not write yourself, above all to `UPDATE_KEYS`, `HOSTS`, `verify_signature` or `fetch_verified_with`: do not sign. Then, in an empty folder (PowerShell; `v0.4.16` stands for the new tag, `v0.4.15` for the one before it):

   ```
   gh release download v0.4.16 --repo kaonaya-studio/uppwind-app --pattern SHA256SUMS --pattern uppwind-companion.exe
   gh attestation verify uppwind-companion.exe --repo kaonaya-studio/uppwind-app --signer-workflow kaonaya-studio/uppwind-app/.github/workflows/release.yml --source-ref refs/tags/v0.4.16
   Get-FileHash uppwind-companion.exe
   Get-Content SHA256SUMS
   ```

   **The keys of the new exe must be exactly yours**, the main one first, and nothing else: a key slipped into `UPDATE_KEYS` (even in second place, next to yours) would let its owner sign every later update alone, and the `--verify-release` checks below would still say `ok`. Compare what the exe itself says with the second line of your two `.pub` files:

   ```
   $main = '<folder of the main key>'; $backup = '<folder of the backup key>'
   $want = "update key 1: $((Get-Content $main\uppwind-update.pub)[1])", "update key 2: $((Get-Content $backup\uppwind-update-backup.pub)[1])"
   $got = @(.\uppwind-companion.exe --update-keys)
   if (($got -join "|") -ceq ($want -join "|")) { "keys ok" } else { "KEYS DIFFER: do not sign"; $got }
   ```

   It must print `keys ok`. (When you change a key on purpose, `$want` is the new list.)

   **What else you check before signing:** the attestation passes and names the commit you wrote down when you created the tag (if not, someone else pushed that tag: do not sign); the SHA-256 of the exe is its line in `SHA256SUMS` (and the .dmg's, when there is one). Never sign a draft you did not start.

   **The strongest check** (recommended; not tried yet, 24 Nov 2026): rebuild the exe yourself from your clone at the tag, as in steps 4 and 5, with the Rust version and the folder path written in the release notes (`D:\a\uppwind-app\uppwind-app`; `subst` can give you a `D:` drive if you have none). Its SHA-256 must equal the draft's. Then your signature covers what you built from code you read, not only what GitHub built. If the two differ, find out why before signing.

   Then sign (minisign asks for the key's password; the comment must be exactly `uppwind-companion` and the tag):

   ```
   minisign -S -s <main>\uppwind-update.key -m SHA256SUMS -t "uppwind-companion v0.4.16"
   ```

   It writes `SHA256SUMS.minisig`. **Check it with the version already published** (the one whose keys the companions have now), then with the new one:

   ```
   <the published uppwind-companion.exe> --verify-release . --release-tag v0.4.16
   .\uppwind-companion.exe --verify-release . --release-tag v0.4.16
   ```

   Both must end with `ok`. Then add the signature and publish:

   ```
   gh release upload v0.4.16 SHA256SUMS.minisig --repo kaonaya-studio/uppwind-app
   gh release edit v0.4.16 --draft=false --latest --repo kaonaya-studio/uppwind-app
   ```

   Last, an installed older companion (0.4.16 or later): `uppwind-companion.exe --check-update --download <an empty folder>` must say `downloaded and checked`. (Not tried yet, 24 Nov 2026: whether `gh release download` and `upload` find a draft by its tag. If they do not, `gh release list` shows the draft, `gh api repos/kaonaya-studio/uppwind-app/releases --jq ".[] | select(.draft) | {id, tag_name}"` gives its id, and the files go through `gh api` with that id.) The first release that carries the check, 0.4.16, is installed by 0.4.15, which ignores the signature: sign it all the same, and use its own exe for the `--verify-release` check.

## The repository's number

Since 0.4.16 the companion asks GitHub for the latest release by the repository's number, `https://api.github.com/repositories/1434113356/releases/latest` (`REPO_ID` and `API_LATEST` in `companion/src/update.rs`), and downloads each file at the address that answer gives for it. It never builds an address from an account name, so renaming the account changes nothing for the companion.

- Renaming the account or the repository, or transferring the repository to another account, keeps the number: nothing to change in the companion. The `--repo` of the `gh` commands above uses the current name; change it there.
- **Never delete the repository to create it again** (nor create a new one to publish from): a new repository has a new number, and every installed companion would stop finding updates, without a way to tell them.
- Check the number before a release: `gh api repos/kaonaya-studio/uppwind-app --jq .id` must print `1434113356`.
- The *releases* link of the status window (`RELEASES_PAGE`) is only opened in the browser, nothing is downloaded from it; GitHub has no web page by repository number, so it names the current account. Change it with the account's name.

## The update keys

The companion accepts updates signed with one of the keys of `UPDATE_KEYS` (`companion/src/update.rs`): at most two, the **main** key and a **backup** key. They are minisign keys (Ed25519; the secret key is encrypted with your password). Their public halves are in the program; their secret halves never touch GitHub, a build machine, a synced folder (Google Drive, OneDrive) or this repository.

- **Creating them** (once, on your own computer; minisign 0.12: `scoop install minisign`, or `minisign-0.12-win64.zip` from github.com/jedisct1/minisign/releases). Keep the two on two different devices (for instance the main one on your computer, outside any synced folder, and the backup one on another machine or a USB stick); `<main>` and `<backup>` below are their two folders:

  ```
  minisign -G -p <main>\uppwind-update.pub -s <main>\uppwind-update.key
  minisign -G -p <backup>\uppwind-update-backup.pub -s <backup>\uppwind-update-backup.key
  ```

  Choose a long password for each (never `-W`, which leaves the key unencrypted) and keep the passwords in a password manager, apart from the key files. The second line of each `.pub` file (it starts with `RW`) goes into `UPDATE_KEYS`, the main one first. A build without any key is refused by `release.yml`, and `AUTO_AVAILABLE` (the *Update automatically* box) is on only when there is one.
- **Replacing a key** (planned): release N carries the new list (for instance the backup key and a new key) and is signed with a key of the list of the version before it; from release N+1 on, sign with a key of the new list. A companion that skipped release N still has the old list: keep a key of the old list valid until you are fine with those companions updating by hand.
- **A key lost**: sign the next release with the other key, and put a new key in its place in that release.
- **A key stolen** (or its password): sign at once, with the other key, a release that removes it, and say so in its notes. Until people install it, the thief can sign updates that their companions accept, though they would also need the GitHub account to publish one. Either key alone can sign any update, so the backup key needs the same care as the main one: a stolen key and the GitHub account together let the thief publish a release that replaces the whole key list, and the companions that install it are theirs for good. Both keys lost: no companion can update itself any more; people download the new version by hand.


## The repository itself

- Commit with the GitHub *noreply* address only, set in this repository (`git config user.email <id>+<login>@users.noreply.github.com`), never globally. Before a push, `git log --format="%an <%ae> | %cn <%ce>"` must show only that address.
- On GitHub: *immutable releases* on (a published release's files and tag cannot change: step 8), *Keep my email addresses private* and *Block command line pushes that expose my email*, two-factor authentication, secret scanning with push protection, Dependabot alerts, private vulnerability reporting (SECURITY.md relies on it).
- CI workflows (`.github/workflows/`, see above): actions pinned by commit SHA, `permissions: contents: read` by default, `contents: write`, `id-token: write` and `attestations: write` only in the job that publishes the release.
- Outside contributions: decide before the first pull request how they are licensed (a DCO, "inbound = outbound", is the simplest), or relicensing later needs every contributor's consent.
