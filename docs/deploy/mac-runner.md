# The Mac runner: building the macOS releases on your own Mac

MoneyKeep's macOS builds (MoneyKeep, CalcKeep, GroupKeep, Form1099Keep and MoneyKeep
Launcher) are made on a Mac you own. On that Mac you install GitHub's runner program. It
waits for work from GitHub and runs each release's `macos` job, after you approve it.
Pull requests are still tested on GitHub's own machines, so code from strangers never
runs on your Mac.

This guide follows `docs/specs/macos-release.md` (Part R). You do steps 1 to 7 once.
Step 8 waits until MoneyKeep Project has an Apple Developer ID (owner decision O1). Until
then, builds are signed "ad hoc": macOS warns the first time someone opens them
(right-click, then **Open**), and MoneyKeep Launcher won't install them.

Do the steps in order. Each one says how to check it before you go on.

---

## 1. The machine

- An Apple Silicon Mac (M1 or later) with macOS 14 or later.
- At least 60 GB free.
- Plugged into mains power, and on a wired network if you can.

**Check:** Apple menu > About This Mac shows an "Apple M…" chip and macOS 14 or later.
System Settings > General > Storage shows at least 60 GB available.

## 2. A separate user, `mkbuild`

The runner, its tools and the signing keychain live only in this user. Don't use your own
account.

1. System Settings > Users & Groups > **Add User…**
2. Type: **Standard** (not Administrator). Full name `mkbuild`, account name `mkbuild`.
   Give it a strong password and keep it in your password manager.
3. Log out, then log in as `mkbuild` for the rest of this guide (unless a step says
   otherwise).

**Check:** in Terminal, as `mkbuild`, `whoami` prints `mkbuild` and `id -Gn` doesn't
include `admin`.

## 3. Power and automatic login

The runner is a per-user background program, so `mkbuild` must be logged in for it to
run.

In System Settings (as an administrator where it asks):

1. Lock Screen (or Battery > Options on a laptop): **Prevent automatic sleeping when the
   display is off** on.
2. Energy (or Battery > Options): **Start up automatically after a power failure** on.
3. Users & Groups > **Automatically log in as**: `mkbuild`.

**FileVault:** with FileVault on, macOS doesn't offer automatic login. Then, after every
restart (an update, a power cut), someone must log in as `mkbuild` by hand before the
runner works again. Release jobs wait in the queue meanwhile (see "When the Mac is
away").

**Check:** restart the Mac. Without FileVault it comes back logged in as `mkbuild`.

## 4. Tools

As `mkbuild`, in Terminal:

1. Apple's command line tools:

   ```sh
   xcode-select --install
   ```

   Click **Install** in the window that opens and wait for it to finish.

2. Rust, from rustup.rs (the stable toolchain), then the two Mac targets:

   ```sh
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   source "$HOME/.cargo/env"
   rustup target add aarch64-apple-darwin x86_64-apple-darwin
   ```

3. `trunk` and the `wasm32-unknown-unknown` target are needed only if a release job asks
   for them. Today none of the `macos` jobs does, so skip them.

You don't need Homebrew.

**Check:**

```sh
xcode-select -p                 # a path, e.g. /Library/Developer/CommandLineTools
rustc --version                 # rustc 1.x.y …
rustup target list --installed  # includes aarch64-apple-darwin and x86_64-apple-darwin
```

## 5. Register the runner with the organisation

1. In a browser, signed in to GitHub as an organisation owner: organisation
   `moneykeepproject` > **Settings** > **Actions** > **Runners** > **New runner** >
   **New self-hosted runner**.
2. Choose **macOS** and architecture **ARM64**. GitHub shows the commands to download
   and configure the runner. Run them as `mkbuild` in Terminal, in a folder such as
   `~/actions-runner`.
3. When you run `./config.sh`, answer the questions like this:
   - runner group: `mac-release` (if you have already made it in step 6; otherwise
     `Default`, and move the runner in step 6);
   - runner name: `mk-mac-1`;
   - additional labels: `mk-mac`;
   - work folder: press Return for the default (`_work`).
4. Install and start it as a service (it starts again on its own when `mkbuild` logs
   in):

   ```sh
   ./svc.sh install
   ./svc.sh start
   ./svc.sh status
   ```

**Check:** on GitHub, Organisation > Settings > Actions > Runners lists `mk-mac-1` as
**Idle**, with the labels `self-hosted`, `macOS`, `ARM64` and `mk-mac`.

## 6. A runner group for the Mac, and the approval environment

### 6a. Runner group `mac-release`

Organisation > Settings > Actions > **Runner groups** > **New runner group**:

- Name: `mac-release`. Add only `mk-mac-1`.
- **Repository access:** Selected repositories: `moneykeep`, `calckeep`, `groupkeep`,
  `form1099keep` and `keep-launcher`.
- **Workflow access:** Selected workflows. Add, for each of the five repositories,
  `moneykeepproject/<repo>/.github/workflows/release.yml@refs/tags/*` and
  `moneykeepproject/<repo>/.github/workflows/release.yml@refs/heads/main` (the second is
  for running a release by hand). For MoneyKeep also add
  `moneykeepproject/moneykeep/.github/workflows/mac-runner-check.yml@refs/heads/main`.
- **Allow public repositories:** on (needed once the repositories are public).

The `Default` group must not contain the Mac. If you registered it in `Default` in
step 5, move it: Runners > `mk-mac-1` > the group menu > `mac-release`.

**If there is no "New runner group" button** (GitHub offers custom runner groups only on
some plans), use the `Default` group instead:

- Open Runner groups > `Default` and set **Repository access** to Selected
  repositories: only the five above. Turn **Allow public repositories** on.
- The protection then rests on two other things, both of which must stay in place:
  the `mac-release` approval environment (6b), so no Mac job runs until you approve it;
  and the workflow check in each repository (`cargo xtask workflows`), which fails if any
  workflow other than `release.yml` (or MoneyKeep's `mac-runner-check.yml`) names a
  self-hosted runner, or if such a workflow can be started by anything other than a
  version tag or a hand run.
- Never add any other repository to `Default` while the Mac is in it.

### 6b. Environment `mac-release` in each repository

Each release's `macos` job waits for your approval (owner decision O3). In **each** of the
five repositories: Settings > **Environments** > **New environment**, name `mac-release`:

- **Required reviewers:** you (the owner). Leave **Prevent self-review** off, or you
  couldn't approve releases you started yourself.
- **Deployment branches and tags:** Selected branches and tags. Add a tag rule `v*` and a
  branch rule `main`.
- No secrets are needed here for the Mac. Only if you want `macos: github` (GitHub's Mac,
  see "When the Mac is away") to sign with a Developer ID, put the `APPLE_CERTIFICATE`,
  `APPLE_CERTIFICATE_PASSWORD`, `KEYCHAIN_PASSWORD`, `APPLE_ID`, `APPLE_PASSWORD` and
  `APPLE_TEAM_ID` secrets here. MoneyKeep's own release notarizes on that path with an App
  Store Connect API key instead of the Apple ID and password, so no password goes on a
  command line: in MoneyKeep's `mac-release` put `APPLE_CERTIFICATE` (the `.p12`, base64),
  `APPLE_CERTIFICATE_PASSWORD`, `APPLE_API_KEY` (the `.p8` key, base64), `APPLE_API_KEY_ID`
  and `APPLE_API_ISSUER`. Without them that path signs ad hoc too.

**Check:** the runner group page lists `mk-mac-1` and the five repositories. Each
repository's Settings > Environments lists `mac-release` with you as reviewer.

## 7. Pull requests from forks

Organisation > Settings > **Actions** > **General**:

- **Fork pull request workflows from outside collaborators:** "Require approval for all
  outside collaborators" (GitHub may word it "all external contributors").
- **Workflow permissions:** "Read repository contents and packages permissions".

**Check:** both settings are saved (each section has its own **Save** button).

## 8. The signing keychain (after the Apple Developer ID; skip until then)

Do this once MoneyKeep Project is enrolled in the Apple Developer Program and you have
the **Developer ID Application** certificate and key as a `.p12` file. Until then, skip
it: builds are signed ad hoc and the release job says so in a warning.

As `mkbuild`, in Terminal. Each command asks for the passwords it needs. None goes on the
command line.

1. Create the keychain with its own new password (keep it in your password manager),
   make it never lock by itself, and add it to the search list next to the login keychain:

   ```sh
   security create-keychain mk-signing.keychain-db
   security set-keychain-settings mk-signing.keychain-db
   security list-keychains -d user -s mk-signing.keychain-db login.keychain-db
   security unlock-keychain mk-signing.keychain-db
   ```

   (`set-keychain-settings` with no options means no lock after a timeout and no lock on
   sleep.)

2. Copy the `.p12` file to the Mac (for example to the Desktop), import it, and let
   `codesign` use the key without asking:

   ```sh
   security import ~/Desktop/developer-id.p12 -k mk-signing.keychain-db -T /usr/bin/codesign
   security set-key-partition-list -S apple-tool:,apple:,codesign: -s mk-signing.keychain-db
   ```

   The first command asks for the `.p12` file's password. The second asks for the
   keychain's password.

3. Store the notarization credentials in the same keychain as the profile `mk-notary`:

   ```sh
   xcrun notarytool store-credentials mk-notary --keychain ~/Library/Keychains/mk-signing.keychain-db
   ```

   It asks for the Apple ID, the team ID and an app-specific password (made at
   account.apple.com > Sign-In and Security > App-Specific Passwords).

4. Delete the `.p12` file from the Mac, and empty the Trash. No Apple secret is stored in
   GitHub.

5. In MoneyKeep Launcher, `catalog::MONEYKEEP_TEAM_ID` is then set to the team ID (a code
   change, done by a contributor). Nothing else changes.

**After every restart:** macOS opens only the login keychain when you log in. Unlock the
signing keychain once as `mkbuild`; it then stays unlocked until the next restart or
logout:

```sh
security unlock-keychain ~/Library/Keychains/mk-signing.keychain-db
```

If you forget, the release job stops with "codesign cannot use the key …" rather than
shipping an unsigned build.

**Check:**

```sh
security find-identity -v -p codesigning mk-signing.keychain-db   # one "Developer ID Application: …"
xcrun notarytool history --keychain-profile mk-notary --keychain ~/Library/Keychains/mk-signing.keychain-db
```

The second command lists past submissions (an empty list is fine). It must not print an
error.

## 9. Check the runner

Run the **Mac runner check** workflow by hand: MoneyKeep repository > **Actions** > "Mac
runner check" > **Run workflow** (branch `main`). Approve it when GitHub asks. It prints
the macOS version, the command line tools, the Rust targets and the free space. Then it
checks the signing identity and the `mk-notary` profile (or reports "ad hoc only" before
step 8), and signs and verifies a small test program. It must pass.

The workflow is `.github/workflows/mac-runner-check.yml`. Its summary lists what it
found; a warning there (less than 20 GB free, the runner not running as `mkbuild`, or
`mkbuild` being an administrator) points at the step to look at again.

## 10. Upkeep

- The runner program updates itself.
- **Monthly**, as `mkbuild`:
  - install macOS updates (System Settings > General > Software Update; this needs an
    administrator's password);
  - `rustup update`;
  - check the free space (`df -h ~`). The build folders in `~/mk-build` can be deleted at
    any time (`rm -rf ~/mk-build`): the next release just builds from scratch. Each one is
    also emptied automatically when Rust changes.
- After a restart: log in as `mkbuild` if FileVault is on, and unlock the signing keychain
  (step 8).

---

## Making a release

1. Push a version tag (`v1.2.3`, matching the version in the repository's `Cargo.toml`).
   This starts `release.yml`.
2. GitHub sends you a notification that the `macos` job is waiting for review. Open the
   run > **Review deployments** > tick `mac-release` > **Approve and deploy**. The other
   platforms don't wait.
3. The job runs on the Mac (up to 2 hours). When every job is done, the draft release has
   the `.dmg` and `SHA256SUMS.txt` (for MoneyKeep also the Windows setup). Publish it as
   usual.

MoneyKeep's phone app is built once on GitHub's Linux machine and handed to the Windows
and Mac jobs, so the Mac needs neither Trunk nor wasm-opt. MoneyKeep's Windows setup is
signed only if the repository has an `MK_SIGN_PFX` secret (a code-signing `.pfx` without
a password, base64); otherwise it is unsigned and the run says so.

A tag push always builds on your Mac. Hand runs (Actions > Release > **Run workflow**)
have a `macos` choice:

| `macos` | What happens |
|---|---|
| `runner` (default) | Built on your Mac, signed from its keychain (or ad hoc before step 8). |
| `github` | Built on GitHub's `macos-15`, signed from the `APPLE_*` secrets in `mac-release` if you added them, otherwise ad hoc. |
| `skip` | No macOS build. The draft release says "No macOS build in this release" at the top. |

## When the Mac is away

If the Mac is off, asleep, logged out or offline, the `macos` job waits in the queue, and
the draft release waits for it. GitHub cancels a job that has waited 24 hours. Don't wait
that long: cancel the run, then start the release by hand (Actions > Release > **Run
workflow**, branch `main`) with `macos` = `github` (GitHub's Mac) or `skip` (no Mac build
this time).

## Why it is set up this way

- **Separate standard user:** a release job can only touch `mkbuild`'s files, never yours,
  and can't change the system.
- **Only release workflows on the Mac:** each repository's `cargo xtask workflows` check
  (run in its CI) fails if any other workflow names a self-hosted runner, or if a release
  workflow can be started by a pull request, a comment or another workflow.
- **Your approval for each Mac job:** even a workflow change that slipped past review can't
  reach the Mac without you clicking Approve.
- **Clean checkout each time:** the job deletes everything untracked in its work folder
  before building. Only the build cache in `~/mk-build` is kept, and only Rust's own
  output goes there.
- **No Apple secrets in GitHub:** the Developer ID key and notarization password stay in
  the Mac's `mk-signing` keychain.
