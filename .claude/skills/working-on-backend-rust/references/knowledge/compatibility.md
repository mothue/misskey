# 互換性・承認・証跡のルール

TS → Rust 移行の目的は「同じ挙動を Rust で再現すること」。Rust の方が正しいと思っても、既存 TS の挙動を勝手に直さない。

## 1. 契約と承認

- 契約は [PORTS.md](../../../../../docs/ts-to-rs/PORTS.md)。承認済みの例外は同ファイルの "Approved compatibility corrections" と [api-kernel-compatibility-plan.md](../../../../../docs/ts-to-rs/api-kernel-compatibility-plan.md) にあるものだけ
- 公開挙動 (status / body / header / エラーの code・id・kind / 副作用の有無と順序) を変える必要が出たら、実装を止めてユーザーに相談する。承認されたら日付つきで上記ドキュメントに記録してから実装する
- 次の設計判断は契約で承認済み。**変えるにはユーザー承認が要る** (性能目的でも同じ):
  - PostgreSQL は接続先 1 つにつき接続 1 本、replica 失敗時に primary へ fallback しない
  - リトライ・TS への fallback・子プロセスの自動再起動をしない
  - legacy callback (TS で実行する業務処理) に汎用タイムアウトを付けない
  - 5 操作の `/rpc` は内部呼び出しと互換性テストのために残す
  - 設定が未対応なら起動を失敗させる (fail closed)

## 2. JS の挙動を再現するコード

- JS / TS / ライブラリ (AJV, fast-querystring, busboy, ioredis, pg など) の挙動を再現する箇所には、**再現元をコメントで書く** (例: `// ratelimiter/microtime.js: Date.now()*1000 起点の単調増加時計`)。「何をしているか」ではなく「なぜこう書くか」を書く
- 再現した挙動ごとに、差分テスト (TS ベースラインとの比較) かフィクスチャを用意する。[behavior.csv](../../../../../docs/ts-to-rs/behavior.csv) に行を足す
- 数値は JS の Number と同じく f64 で扱う。NaN / Infinity / -0 を JSON に変換する前に判定へ使う値は、変換前の f64 で持つ
- 公開リクエスト・レスポンスの値は `api_validation::Value` (UTF-16 文字列、undefined と null の区別、プロパティ順を保持) で扱う。`serde_json::Value` に落とすのは、内部設定や DB の JSON など JS の値ではないものだけ

## 3. TS と共有する一覧

TS 側が持つ一覧を Rust にコピーすると、TS 側の変更で黙ってずれる。次は手書きで複製しない。

| 一覧 | 正しい扱い |
| --- | --- |
| endpoint 名と meta / params | bootstrap の `api.endpoints` から受け取る |
| Rust native の endpoint 名 | Rust 側の dispatch 表を唯一の定義にし、TS は bootstrap 応答や設定で受け取る (TS 側の `['ping', 'test', ...]` 直書きを増やさない) |
| special route | 同上。片側にしか書かない |
| policy 名と集約方法 | TS から受け取る。受け取れない間は、`default_policies` に Rust の知らないキーがあれば `AuthService::new` で起動失敗させる |
| entity のテーブル・カラム | bootstrap の `entityLayouts` から受け取る |
| 公開エラー (message / code / id / kind) | TS の `ApiError` 定義と 1 文字も違わないこと。Rust 側は 1 つのモジュールに定数として並べる |

どうしても Rust 側に一覧を書く場合は、**一覧にない値が来たら起動を失敗させる**。`_ => continue` / `_ => return None` で読み飛ばすのは禁止。

## 4. 入力の受け付け

- TS から受け取る設定 (bootstrap) と TS ↔ Rust 間のフレームは `#[serde(deny_unknown_fields)]` で厳しく受ける
- 公開リクエストは、旧 Fastify / TS が受け付けたものを同じように受け付ける。厳しくしすぎるのも互換性違反
- Redis の購読チャンネルには業務イベントも流れる。`internal` 以外のメッセージは中身を解析せずに無視する。解析できないメッセージ 1 件で認証を止めない

## 5. 証跡

- 検証結果は [api-kernel-validation.md](../../../../../docs/ts-to-rs/api-kernel-validation.md) など既存の evidence ファイルに PASS / FAIL / BASELINE / SKIPPED で記録する。前回の成功を流用しない
- endpoint 数などの数は観測値として書く。コード上の定数にしない
