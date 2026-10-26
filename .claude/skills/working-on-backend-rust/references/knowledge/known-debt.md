# 既存コードの既知の違反 (真似しない)

2026-10-24 のレビュー時点で、既存コードには規約に反する箇所がある。新しいコードの手本にしない。
直すときは、挙動を変えないリファクタと挙動変更を別コミットにする。直したらこの一覧から消す。

## 互換性に関わるもの (優先して直す)

| 場所 | 内容 | 規約 |
| --- | --- | --- |
| `api-auth/src/role.rs` の `policy_aggregation` | policy 名と集約方法を直書き。未知の policy を `continue` で黙って捨てる | compatibility.md 3 章 |
| `api-runtime/src/redis.rs` の購読ループ | 解析できないメッセージ 1 件で `ready=false` になり、認証付き API が止まる。契約の「無関係なメッセージは無視する」に反する | compatibility.md 4 章 |
| `api-runtime/src/lib.rs` の `process` / `TS ApiServerService.executeLegacy` | special route と native endpoint の一覧を Rust と TS の両方に直書き | compatibility.md 3 章 |
| `api-runtime/src/response.rs` の `framework` と `lib.rs` の `FrameworkError` 処理 | 同じ JSON を 2 か所で作り、文言がないときの値が `"Internal Server Error"` と `"Error"` で違う | rust-style.md 6 章 |
| `api-runtime/src/lib.rs` の `CallbackResult::Authentication` | 旧実装の 401 ではなく 500 になる (現在は通らない分岐) | compatibility.md 1 章 |
| backend-rust 全体 | `tracing` が依存にあるのに未使用。失敗時の出力は 1 行だけ | rust-style.md 1 章 |

## 書き方のもの

| 場所 | 内容 | 規約 |
| --- | --- | --- |
| 5 つの crate の `Failure` | 中身のない unit struct。`From<()>` と `.map_err(\|_\| ())` | rust-style.md 1 章 |
| `api-auth/src/lib.rs` の `PolicySnapshot` | `Deref<Target = Value>` で数値判定を JSON 経由でも書けてしまう | rust-style.md 2 章 |
| `api-runtime/src/lib.rs` の `flag(&meta, "...")` | endpoint の meta を文字列キーで参照 | rust-style.md 2 章 |
| `api-runtime/src/response.rs` の `api_error` | `&str` が 4 つ並ぶ引数。エラー定義が `authorize` に直書き | rust-style.md 2 章 |
| `api-validation/src/value.rs` の `Value::as_str` | `Option<String>` を新しく作って返す。`stringify` と `to_json_string` が別名 | rust-style.md 3 章 |
| `api-runtime/src/request.rs` の `Input._temporary` | 使っているフィールドに `_` | rust-style.md 3 章 |
| `api-validation/src/value.rs` の `Value::from_json` | 再帰で辿る | rust-style.md 4 章 |
| `api-auth/src/lib.rs` の `AuthConfig` / `Context` | `Clone` / `Drop` を手書き (`SafeJson` で不要になる) | rust-style.md 4 章 |
| `api-validation/src/schema.rs` | JSON のまま解釈し `expect` / `unreachable!` / `self.patterns[..]` に依存。324 行の関数 | rust-style.md 4・6 章 |
| `api-auth/src/lib.rs` の `AuthService` | ロック中に await しないのに `tokio::sync::Mutex` | rust-style.md 5 章 |
| `api-runtime/src/redis.rs` の `connection` | `MultiplexedConnection` を Mutex で包み、コマンド中もロックを持つ | rust-style.md 5 章 |
| `api-runtime/src/lib.rs` の `log_ip`、`api-auth/src/lib.rs` の `update_last_used` | `tokio::spawn` の投げっぱなし | rust-style.md 5 章 |
| `storage-postgres/src/lib.rs` の `count_at` | 1ms の `sleep` でポーリング。`Database::query` とほぼ同じコードの重複 | rust-style.md 5・6 章 |
| `api-runtime/src/lib.rs` の `process` / `execute` / `authorize` | 100 行超。native handler の本体が `execute` の中にある | rust-style.md 6 章 |
| `bridge/src/main.rs` の認証チェック | 2 つのハンドラにコピー。TS 側と違い定数時間比較でない | rust-style.md 6 章 |
| `api-auth/src/postgres.rs` | SQL の長い文字列リテラルが関数内に直書き | rust-style.md 6 章 |
| `api-auth/src/lib.rs` の `AuthService::new` | 使わない引数 `_now_ms` | rust-style.md 3 章 |
