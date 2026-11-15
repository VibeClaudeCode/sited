# sited
ssso

## Swiftgram Tontoho mode → .ipa

`patches/tontoho-mode.patch` is applied on top of Swiftgram/Telegram-iOS and built by
`.github/workflows/build-ipa.yml` on a macOS GitHub runner.

1. (Optional) Settings → Secrets and variables → Actions: add `TG_API_ID` and `TG_API_HASH` (from https://my.telegram.org/apps). Without them the stock Swiftgram keys are used.
2. Actions → "Build Swiftgram Tontoho IPA" → Run workflow.
3. Download the `Swiftgram-Tontoho-ipa` artifact when the run finishes. The .ipa is unsigned (fake codesigning); sign it yourself (AltStore, Sideloadly, etc.) to install.
