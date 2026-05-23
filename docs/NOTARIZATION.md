# Code signing & notarization

Colomin's release workflow can sign and notarize the `.app` and the `.dmg`
using any Apple Developer ID. The signing/notarization steps in
`.github/workflows/release.yml` are conditional on six GitHub secrets — if
they're unset, the release still builds and publishes (unsigned, with the
usual Gatekeeper warning). When the secrets are present, signing activates
automatically.

This document covers the one-time setup that the Developer ID holder
(the "signer") has to do.

## Switching Developer IDs later

All of the secrets are scoped to a single Apple Developer account. To move
to a different signer (e.g. from a friend's account to your own), just
re-do the steps below with the new account's cert and API key, then update
the six secrets. Nothing about Homebrew, the DMG URL, the cask, or already-
installed copies needs to change — each release is independent.

The one user-visible difference is the developer name in the Gatekeeper
dialog and `codesign -dv` output.

## What the signer needs

The signer is whoever's Apple Developer account ($99/year) is being used.
They need to give you:

1. **A `Developer ID Application` certificate**, exported as `.p12` with a password.
2. **An App Store Connect API key** (`.p8`), plus its Key ID and Issuer ID.
3. **Their Team ID** (10 chars, e.g. `AB12CDE34F`).

The App Store Connect API key is preferred over Apple ID + app-specific
password — it has no MFA prompts and can be scoped to a single role.

## Step 1 — Export the Developer ID Application cert

On the signer's Mac (the cert lives in their login keychain):

1. Open **Keychain Access** → **login** keychain → **My Certificates**.
2. Find **`Developer ID Application: <Name> (<TEAMID>)`**.
3. Right-click → **Export** → save as `colomin-signing.p12`.
4. Set a strong export password. **Remember it** — you'll need it as
   `APPLE_CERT_PASSWORD`.

If no Developer ID Application cert exists yet, create one:

1. Sign in at <https://developer.apple.com/account/resources/certificates>.
2. Click **+** → choose **Developer ID Application** → follow the CSR steps.
3. Download the `.cer`, double-click to add to Keychain Access, then export
   as `.p12` per above.

Base64-encode the `.p12` for the GitHub secret:

```bash
base64 -i colomin-signing.p12 | pbcopy
```

The clipboard now holds the value for `APPLE_CERT_P12_BASE64`.

## Step 2 — Create an App Store Connect API key

1. Sign in at <https://appstoreconnect.apple.com/access/integrations/api>.
2. Tab: **Team Keys** (under "Integrations" → "App Store Connect API").
3. Click **+** → name it `colomin-notarize` → role: **Developer**.
4. Generate the key. **Download the `.p8` file immediately** — Apple only
   lets you download it once.
5. Note the **Key ID** (visible next to the key in the list, 10 chars) and
   the **Issuer ID** (header text at the top of the keys page, a UUID).

Base64-encode the `.p8`:

```bash
base64 -i AuthKey_XXXXXXXXXX.p8 | pbcopy
```

The clipboard now holds the value for `APPLE_API_KEY_BASE64`.

## Step 3 — Set the six GitHub secrets

On the `saman/colomin` repo:

```bash
gh secret set APPLE_CERT_P12_BASE64 --repo saman/colomin   # paste from step 1
gh secret set APPLE_CERT_PASSWORD   --repo saman/colomin   # .p12 export password
gh secret set APPLE_TEAM_ID         --repo saman/colomin   # 10-char team id
gh secret set APPLE_API_KEY_BASE64  --repo saman/colomin   # paste from step 2
gh secret set APPLE_API_KEY_ID      --repo saman/colomin   # 10-char key id
gh secret set APPLE_API_ISSUER_ID   --repo saman/colomin   # uuid issuer id
```

That's it. The next `./scripts/release.sh <version>` will produce a signed
and notarized DMG; Gatekeeper will accept it without warnings.

## How it works

`.github/workflows/release.yml`:

1. Imports the `.p12` into an isolated keychain on the runner.
2. Extracts the identity name via `security find-identity` and stashes it
   in a step output.
3. Writes the `.p8` API key to a temp path.
4. After bundling the `.app`:
   - `scripts/sign-app.sh target/release/Colomin.app` — codesigns with
     hardened runtime + secure timestamp.
   - `scripts/notarize.sh target/release/Colomin.app` — submits via
     `notarytool submit --wait`, then staples the ticket.
5. After building the DMG: same two scripts on the `.dmg`.

The DMG ships with both itself and the bundled `.app` stapled, so first
launch works offline without contacting Apple.

## Verifying the result

After a successful signed release, download the DMG and run:

```bash
# DMG ticket present and valid
xcrun stapler validate Colomin.dmg

# Mount, then validate the app inside
hdiutil attach Colomin.dmg
xcrun stapler validate "/Volumes/Colomin <version>/Colomin.app"
spctl --assess --type execute --verbose "/Volumes/Colomin <version>/Colomin.app"
hdiutil detach "/Volumes/Colomin <version>"
```

`spctl` should report `accepted source=Notarized Developer ID`.

## After the first signed release

Once a signed release ships, the unsigned-app warnings in `README.md`
become outdated. Trim the "first launch will be blocked by Gatekeeper"
paragraph and the `xattr -dr com.apple.quarantine` workaround from the
**Install → Prebuilt DMG** section.

## Troubleshooting

**`No "Developer ID Application" identity found in imported cert.`**
The `.p12` doesn't contain a Developer ID Application cert (it might be a
Mac Developer or Mac App Distribution cert instead). Re-export from
Keychain Access making sure the right cert is selected.

**`The signature of the binary is invalid` from notarytool**
Usually means the `.app` was modified after signing. The workflow signs
just before notarization, so this typically indicates a stale build. Push
a clean tag and re-run.

**Notarization fails with "team is not enrolled in the Apple Developer Program"**
The signing cert and the API key must belong to the same team. Re-check
that `APPLE_TEAM_ID` matches the Team ID in the cert's common name and the
API key's owning team.

**`security: SecKeychainItemImport: ... (-25257)`**
The `.p12` password is wrong. Re-export the cert with a known password.
