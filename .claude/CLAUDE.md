# Claude Configuration

- 日本語で回答してください

## ブランチ戦略

- ブランチ名のプレフィックス: Claude Code が作業するブランチ名の prefix に claude/ をつける
  - 作業しているブランチ名のprefixが異なる場合はrenameしてください
- Pull Request のラベル: Claude Code が作成したコードの Pull Request に claude code のラベルをつける

## プロトコル仕様

- Media over QUIC Transport(MoQT)関連の仕様はspecディレクトリ配下に格納されています。必要であれば仕様の確認をしてから回答・実装を行なってください

## linter

- JS: `npx prettier --check`
- Rust: `cargo clippy` `cargo fmt`
