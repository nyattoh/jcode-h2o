# Understood Errors & Failure Modes

## Error Patterns
### [Cargo rustc-wrapper on Windows]
- **Cause**: ルート `.cargo/config.toml` に `rustc-wrapper = "scripts/rustc-parallel-frontend"`（シェルスクリプト）が指定されているため、Windows PowerShellで直接 cargo を叩くと `os error 193` が発生する。また PowerShell 5.1 で `$env:RUSTC_WRAPPER=""` とすると変数が削除されてしまい効かない。
- **Solution**: Git Bash から `export RUSTC_WRAPPER= && cargo ...` で実行するか、PowerShell から `cargo ... --config 'build.rustc-wrapper=\"\"'`（または PowerShell スクリプト経由）で実行する。
- **Prevention**: Windows 環境でのビルドコマンド実行時は常に Git Bash 経由で実行する。

## Known Failure Modes
### [Optional Dependency Git Fetch in CI]
- **What looks correct**: `experiments/h2o-drift/Cargo.toml` に新しい依存関係を気軽に追加すること。
- **Why it's wrong**: CI で `--locked` が指定されており、不要な features が有効化されると未ダウンロードの crate や重厚依存（例: agentgrep 等）の fetch/compile が発生し CI が壊れる。
- **Correct approach**: `embedded-runtime` を `native-ui` から分離し、独立したオプトイン feature とする。CI では `--features "native-ui jev"` のみ指定されるため、`embedded-runtime` の依存は解決・コンパイルされず影響を受けない。
