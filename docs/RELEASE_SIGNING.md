# Signed macOS releases

The warning documented on the v0.1.1 release is a macOS Gatekeeper issue: the
DMG was not signed with an Apple Developer ID certificate and was not
notarized. A GitHub "Verified" commit or tag signature does not sign the app
bundle. The distributed `Memex.app` / `.dmg` must be built with Apple signing
credentials and submitted to Apple's notarization service.

## Required GitHub Actions secrets

Configure these repository secrets before running
`.github/workflows/release-macos.yml`:

| Secret | Purpose |
|---|---|
| `APPLE_CERTIFICATE` | Base64-encoded `.p12` export of the Developer ID Application certificate. |
| `APPLE_CERTIFICATE_PASSWORD` | Password used when exporting the `.p12`. |
| `APPLE_SIGNING_IDENTITY` | Optional exact keychain identity, e.g. `Developer ID Application: ... (TEAMID)`. If omitted, CI picks the first Developer ID Application identity. |
| `APPLE_ID` | Apple ID email used for notarization. |
| `APPLE_PASSWORD` | App-specific password for the Apple ID. |
| `APPLE_TEAM_ID` | Apple Developer Team ID. |
| `KEYCHAIN_PASSWORD` | Temporary CI keychain password. |

Create `APPLE_CERTIFICATE` with:

```bash
openssl base64 -A -in DeveloperIDApplication.p12 -out DeveloperIDApplication.p12.base64
```

Paste the contents of `DeveloperIDApplication.p12.base64` into the secret.

## Release flow

1. Bump `package.json`, `src-tauri/Cargo.toml`, and
   `src-tauri/tauri.conf.json`.
2. Create a new patch tag, for example `v0.1.2`.
3. Run **Release macOS signed DMG** from GitHub Actions, or push the tag.
4. The workflow builds with:

```bash
npm run tauri:release:macos
```

That command builds only the Apple Silicon DMG and passes
`--no-default-features --features gui` so the shipped app keeps the desktop GUI
but does not include the WebKit Inspector devtools feature.

The workflow then verifies:

```bash
codesign --verify --verbose=2 Memex_*.dmg
spctl --assess --type open --context context:primary-signature --verbose=4 Memex_*.dmg
xcrun stapler validate Memex_*.dmg
```

If those checks pass, the uploaded DMG should not show the "damaged because it
cannot be checked for malicious software" first-launch failure.

## Notes

- Prefer a new patch tag over replacing an existing public artifact. Replacing
  `v0.1.1` with a newly signed DMG is technically possible, but a new tag makes
  provenance clearer and avoids mutating a version that users may already have
  downloaded. The workflow intentionally does not use `gh release upload
  --clobber`.
- Tauri updater signing is separate from Apple Developer ID signing. If Memex
  later enables `tauri-plugin-updater`, also configure
  `TAURI_SIGNING_PRIVATE_KEY` and the updater public key in `tauri.conf.json`.
