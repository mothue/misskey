---
name: working-on-backend-rust
description: Use whenever editing or adding code under `packages/backend-rust/`, or TS code that talks to it (`packages/backend/src/server/rust/`, `ApiServerService.ts`, `RustBackendService.ts`, `docs/ts-to-rs/`). Covers the TS-to-Rust migration contract (behavior compatibility first, approval rules, evidence), Rust coding rules (error types, typed data, async/locking, deep JSON, naming), TS bridge-side rules, porting a TS handler to native Rust, and the cargo validation commands CI runs. Must be consulted before any Rust or Rust-bridge change. This is NOT waived by having already invoked working-on-backend, brainstorming, writing-plans, or any other upstream skill.
---

# working-on-backend-rust

Misskey backend の TS → Rust 移行で、`packages/backend-rust/` と TS 側ブリッジを触るときの規約。
**最優先は「既存 TS と観測可能な挙動が同じであること」。性能改善・リファクタはその後。**

SKILL.md は索引と絶対ルールだけ。詳細は references を Read すること。

## 絶対ルール (違反 = 差し戻し)

1. [docs/ts-to-rs/PORTS.md](../../../docs/ts-to-rs/PORTS.md) が移行の契約。契約と挙動を変える変更は、ユーザーの明示承認なしに実装しない
2. TS が持つ一覧 (endpoint 名・special route・policy 名と集約方法・テーブル名・エラー定義) を Rust に手書きで複製しない。bootstrap で受け取るか、未知の値で起動失敗させる。**未知の値を黙って読み飛ばさない**
3. エラーの中身を捨てない。`.map_err(|_| ())` / `From<()>` / 中身のない `Failure` を新しく足さない。失敗は種類付き enum にし、`tracing` で「どの操作・どの種類」を記録する (秘密・SQL・token・entity の値は出さない)
4. 公開データを再帰で辿らない。深い JSON は既存の反復実装 (`SafeJson` / `clone_json` / `drop_json` / `api_validation::Value`) を使い、手書きの `Clone` / `Drop` を新しく足さない
5. ロック中に `.await` しないなら `std::sync::Mutex`。`tokio::spawn` の投げっぱなし・`sleep` ポーリングを足さない
6. 既存コードの既知の違反を真似しない ([known-debt.md](references/knowledge/known-debt.md))。触った関数は規約に寄せてよいが、挙動を変えないリファクタは挙動変更と別コミットにする

## 作業別ワークフロー (tasks)

- TS の endpoint / 処理を Rust native に移す → [references/tasks/porting-handler.md](references/tasks/porting-handler.md)

## 規約 (knowledge)

- 互換性と承認・証跡のルール → [references/knowledge/compatibility.md](references/knowledge/compatibility.md)
- Rust のコード規約 (良い例 / 悪い例つき) → [references/knowledge/rust-style.md](references/knowledge/rust-style.md)
- TS ブリッジ側の規約 → [references/knowledge/ts-bridge.md](references/knowledge/ts-bridge.md)
- 既存コードの既知の違反一覧 → [references/knowledge/known-debt.md](references/knowledge/known-debt.md)

## 検証コマンド

`packages/backend-rust/` で実行する。CI ([backend-rust.yml](../../../.github/workflows/backend-rust.yml)) と同じ。

```bash
cargo fmt --all --check
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo test --locked --workspace
```

DB / Redis が要るテストは CI と同じく `-- --ignored` / `--include-ignored` で別に回す。
加えて、変更した関数で次の lint の警告を新しく増やさない (既存の警告は対象外):

```bash
cargo clippy --locked --workspace --lib --bins -- \
  -W clippy::too_many_lines -W clippy::fn_params_excessive_bools \
  -W clippy::cast_possible_truncation -W clippy::needless_pass_by_value \
  -W clippy::significant_drop_tightening -W clippy::match_same_arms
```

TS 側を触ったら `working-on-backend` スキルの手順と ESLint も通す。

## 必ず最後に通る場所

commit / PR / 作業の引き渡し前に [shipping-misskey-change](../shipping-misskey-change/SKILL.md) に従う。
