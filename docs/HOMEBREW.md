# Homebrew distribution

Colomin is distributed via a Homebrew Cask in the [`saman/homebrew-tap`](https://github.com/saman/homebrew-tap) repo. End users install it with:

```bash
brew install --cask saman/tap/colomin
```

The cask is bumped automatically by the release workflow whenever a `v*` tag is pushed. This document covers the one-time setup that the maintainer has to do.

## One-time setup

### 1. Create the tap repo

The tap repo lives at `github.com/saman/homebrew-tap`. The name **must** start with `homebrew-` for `brew tap saman/tap` to resolve it.

```bash
gh repo create saman/homebrew-tap --public \
  --description "Homebrew tap for Saman's apps"
```

Seed it with the current cask (so `brew install` works before the first auto-bump runs):

```bash
git clone https://github.com/saman/homebrew-tap.git /tmp/tap
mkdir -p /tmp/tap/Casks
./scripts/update-cask.sh --print <current-version> <current-dmg-sha256> \
  > /tmp/tap/Casks/colomin.rb
cd /tmp/tap
git add Casks/colomin.rb
git commit -m "colomin <current-version>"
git push
```

To get the current DMG sha256 for the seed commit:

```bash
gh release view --repo saman/colomin --json assets \
  --jq '.assets[] | select(.name == "Colomin.dmg") | .digest'
```

### 2. Add the `TAP_TOKEN` secret

The release workflow needs a token with `contents: write` on the tap repo. A fine-grained personal access token is recommended.

1. Visit https://github.com/settings/personal-access-tokens/new
2. **Resource owner:** `saman`
3. **Repository access:** only select `saman/homebrew-tap`
4. **Permissions → Repository → Contents:** Read and write
5. Copy the generated token

Add it as a secret on the `saman/colomin` repo:

```bash
gh secret set TAP_TOKEN --repo saman/colomin
# paste the token when prompted
```

That's it. The next `git push origin v*` tag will publish a release and bump the cask.

## How the auto-bump works

`.github/workflows/release.yml` runs `scripts/update-cask.sh <version> <sha256>` after the DMG is uploaded to the GitHub Release. The script:

1. Clones `saman/homebrew-tap` using `TAP_TOKEN`.
2. Regenerates `Casks/colomin.rb` from the embedded template.
3. Commits and pushes if anything changed.

If `TAP_TOKEN` is missing, the script logs a skip notice and exits 0 — releases still publish, just without the cask bump.

## Manual operations

Render the cask formula locally without pushing:

```bash
./scripts/update-cask.sh --print 0.2.0 <sha256>
```

Force a cask bump outside of a release (e.g. after fixing the formula):

```bash
TAP_TOKEN=<token> ./scripts/update-cask.sh 0.2.0 <sha256>
```

Sanity-check the published cask:

```bash
brew tap saman/tap
brew info --cask colomin
brew install --cask colomin
```

## Submitting to homebrew-cask (main tap)

The personal tap is the right home until Colomin meets the upstream
[notability criteria](https://docs.brew.sh/Acceptable-Casks) — roughly:

- Repo with ~30 forks, ~30 watchers, or ~75 stars.
- Stable, versioned releases (we have these).
- Ideally code-signed and notarized binaries — see [`NOTARIZATION.md`](NOTARIZATION.md) for the wiring in the release workflow. Until those secrets are set, builds ship unsigned and Gatekeeper warns on first launch, which is also a likely upstream rejection reason.

Once those land, the cask can be PR'd to `homebrew/homebrew-cask` and the personal tap can either be kept (for pre-release builds) or deprecated.
