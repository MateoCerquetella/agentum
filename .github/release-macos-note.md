
---

### 📦 Installing the macOS desktop app

**Homebrew (recommended — installs with no Gatekeeper warning):**

```sh
brew tap MateoCerquetella/tap
brew trust MateoCerquetella/tap   # one-time — lets the cask clear the quarantine flag
brew install --cask agentum
```

**Direct `.dmg` download:** Agentum is not Apple-notarized yet, so macOS shows an
"app is not verified" / "unidentified developer" warning on first launch. After
moving **Agentum.app** into your Applications folder, clear the quarantine flag
with one command, then open it normally:

```sh
xattr -dr com.apple.quarantine /Applications/Agentum.app
```
