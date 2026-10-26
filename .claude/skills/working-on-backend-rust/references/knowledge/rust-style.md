# Rust コード規約

workspace は `packages/backend-rust/` (edition 2024、Rust 1.99.0 固定)。`cargo fmt` と CI の clippy (`-D warnings`) は前提で、ここではそれ以上の規約を定める。

## 1. エラー

失敗の種類は残し、秘密は残さない。

```rust
// 悪い: 何が起きたか失われる
.map_err(|_| ())?;
.map_err(|_| Failure)?;   // Failure が中身のない struct

// 良い: 種類だけ持つ。SQL・token・接続文字列は持たない
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    #[error("connect")] Connect,
    #[error("timeout")] Timeout,
    #[error("query")] Query,
    #[error("decode")] Decode,
    #[error("stopped")] Stopped,
}
.map_err(|_| StoreError::Query)?;
```

- crate ごとにエラー型を 1 つ定義する。同じ名前の `Failure` を crate ごとに作らない
- `impl From<()> for ...` と `Result<_, ()>` は使わない
- HTTP レスポンスへの変換は境界 (`api-runtime` の dispatch) の 1 か所で行う。下の層から `Response` をエラーとして返す形 (`Result<_, Box<Response<Body>>>`) を新しく増やさない
- 境界では `tracing` で操作名とエラーの種類を記録する。値・SQL・token・entity を `Debug` / `Display` で出さない

## 2. 型

- 取りうる値が決まっている文字列は enum にする (`kind: &str` → `enum InvocationKind`)
- 公開関数に `bool` 引数を足さない。`query(read: bool)` → `query(Access::Read)`
- Rust が読む endpoint の meta は、起動時に型付き struct に変換する。`flag(&meta, "allowGet")` のような文字列キーでの参照を増やさない
- 引数が同じ型で並ぶ関数 (`api_error(message, code, id, kind, ...)`) を作らない。struct の定数にまとめる:

```rust
pub struct ApiErrorDef { pub message: &'static str, pub code: &'static str, pub id: &'static str, pub kind: ErrorKind }
pub const ACCESS_DENIED: ApiErrorDef = ApiErrorDef { message: "Access denied.", code: "ACCESS_DENIED", id: "56f35758-7dd5-468b-8439-5d6fb8ec9b8e", kind: ErrorKind::Client };
```

- 型の安全を逃がす `Deref` を付けない。スマートポインタでない型に `Deref` を実装するのは、意図的に中身へのアクセスを全部許す場合だけ (`PolicySnapshot` のように「数値判定には別メソッドを使え」という型には付けない)
- 参照の Option は `Option<&T>`。`&Option<T>` を引数・フィールドにしない

## 3. 命名

- `as_*` は割り当てなしで借用を返す。新しく割り当てるなら `to_*`、所有権を移すなら `into_*`
- 同じ処理に 2 つの公開名を付けない (`stringify` と `to_json_string` のような別名)
- 使っているフィールドに `_` を付けない。Drop のためだけに持つ値は役割が分かる名前にしてコメントを書く (`temp_file: TempPath // drop で一時ファイルを消す`)
- 同じファイルで `serde_json::Value` と `api_validation::Value` の両方を使うなら、`serde_json::Value` を `Json` として import する
- ローカル変数名で同じモジュールの関数を隠さない (`|error|` が関数 `error` を隠して `super::schema::error` と書く羽目になる、など)
- 型の名前を用途名にしない。汎用の接続プールを `OnlineUsers` と呼ばない

## 4. 深いデータと panic

- 公開入力・DB の JSON・Redis のイベントは任意の深さがありうる。再帰で辿らない。既存の反復実装を使う
- 深い JSON を持つ struct は、フィールドを `SafeJson` にする。`Clone` / `Drop` を手書きしない
- 非テストコードで `unwrap()` を使わない。`expect()` は「型や直前の検査で保証された不変条件」にだけ使い、メッセージに不変条件を書く
- 不変条件は型で表すのを優先する。`expect("compiled schema")` / `unreachable!()` に頼る設計を増やさない
- 数値の変換は `as` ではなく `try_from` / `from`。値の範囲が論理的に保証されていて `as` を使うなら、理由をコメントに書く

## 5. 非同期・ロック・タスク

- ロック中に `.await` しないなら `std::sync::Mutex` / `RwLock`。`tokio::sync::Mutex` はロックしたまま `.await` する必要があるときだけ
- ネットワーク I/O の間ロックを持ち続けない。持つ必要がある (契約上 1 接続を直列に使う、など) なら理由をコメントに書く
- `MultiplexedConnection` のように clone して並行に使う前提の型を Mutex で包まない
- `tokio::spawn` の戻り値を捨てない。`TaskTracker` / `JoinSet` で追跡し、shutdown で待つ
- `sleep` でのポーリングをしない。`Notify` / `watch` / `JoinSet` で待つ
- 待ち処理は shutdown の合図 (`watch`) で中断できるようにする。`tokio::select!` で shutdown を優先するときは `biased;` を付ける

## 6. 構造

- 関数は 80 行程度を目安にする。超えるなら分ける
- native handler は 1 つにつき 1 関数 (できれば 1 モジュール) にし、dispatch 表から呼ぶ。`execute` の `match` に処理本体を直接書かない
- 似た処理をコピーしない。差分は引数で表す (例: `count_at` と `Database::query` の重複を増やさない)
- 制限値・サイズ・回数は名前付き定数にする (`const MULTIPART_FIELD_LIMIT: usize = 1024 * 1024;`)。`req_packed_commands(&pipeline, 8, 1)` のような意味の読めない数値を直接書かない
- SQL は生文字列の `const` に出す。値は必ずパラメータで渡す。識別子は bootstrap の `entityLayouts` から来たものだけを `quote_identifier` で埋め込む

## 7. trait と依存

- 新しい trait の `async fn` は、`dyn` で使わないなら `async_trait` なしで書く
- 依存 crate を増やすときは理由を PR に書く。既にある機能と重複する crate (別の JSON 実装・別の TLS 実装など) を足さない
- `core` は標準ライブラリだけに依存する (契約)

## 8. テスト

- JS 互換の挙動は、TS 側の実際の出力から作ったフィクスチャで確かめる (`tests/fixtures/`)。期待値を手で書かない
- DB / Redis が要るテストは `#[ignore]` を付け、CI の `--ignored` ジョブで回す
- テストの中では `unwrap()` を使ってよい

## 9. コメント

- 書くのは「なぜ」。特に再現している JS / TS / ライブラリの挙動と、その出典
- コードを言い換えるだけのコメントは書かない
