# TS ブリッジ側の規約

対象: `packages/backend/src/server/rust/`、`ApiServerService.ts`、`ApiCallService.ts` の Rust 関連部分、`RustBackendService.ts`。
通常の backend 規約 (`working-on-backend` スキル) に加えて、次を守る。

## 1. lint と書式

- 変更した TS ファイルで ESLint の警告を新しく増やさない。特に次は出しやすい:
  - `@typescript-eslint/no-non-null-assertion`: `tasks.pop()!` ではなく `for (let task; (task = tasks.pop()) !== undefined;)` のように書く
  - `import/order`: `@/` の import は外部パッケージの後、相対 import と `import type` はその後
  - `@typescript-eslint/no-unused-vars`: 使わない引数は `_options` のように `_` を付ける
  - `@typescript-eslint/no-unnecessary-condition`: null にならない値に `??` / `?.` を付けない
- `brace-style` は 1 行ブロックを許すが、**複数の文を 1 行のブロックに詰めない** (`if (!x) { cleanup(); return y; }` は 3 行に分ける)。1 行に収めてよいのは `if (x) return;` のような単文だけ
- 1 行が長くなる関数呼び出し・オブジェクトリテラルは、引数・プロパティごとに改行する
- NestJS のコンストラクタは [nestjs-di.md](../../../working-on-backend/references/knowledge/nestjs-di.md) の形にする (`@Inject(DI.xxx)` を 1 行、`private` 宣言を次の行、依存の間は空行)

## 2. Rust との境界

- Rust から来たデータは `unknown` として受け、明示的な検査で型を絞る。`as` キャストで検査の代わりにしない
- 型ガード (`object` など) は `protocol.ts` のものを使う。ファイルごとに書き直さない
- Rust と TS の両方が知る必要がある一覧は、片方だけに書いて bootstrap で渡す ([compatibility.md](compatibility.md) の 3 章)
- 公開エラーは既存の `ApiError` 定義を使い、message / code / id を新しく書き起こさない

## 3. エラー処理

- `ApiError` 以外の例外は、旧 `ApiCallService.#onExecError` と同じく error id を付けてログに出してから汎用エラーにする。ログなしで握りつぶさない
- 起動時の設定の変換 (Redis / PostgreSQL など) は、旧 TS が受け付けた設定で起動失敗しないこと。未対応の設定で失敗させる場合は、どの設定が未対応かをログに出す
- `catch {}` / `catch { return null; }` で原因を捨てない

## 4. ライフサイクル

- `??=` で Promise をキャッシュするときは、失敗した Promise を残さない (再試行できない状態を作らない)。意図して再試行させないなら、そうコメントに書く
- 送信済みのレスポンスに対して例外を投げない (ストリームが途中で失敗したときは接続を閉じるだけにする)
