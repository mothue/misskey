# TS の処理を Rust native に移す

TS で実行している endpoint (legacy callback) を Rust の native handler に移すときの手順。
移す範囲そのものが契約 ([PORTS.md](../../../../../docs/ts-to-rs/PORTS.md)) の変更になるので、**着手前にユーザーの承認を取る**。

## 1. 観測できる挙動を洗い出す

- TS の endpoint 本体と、そこから呼ばれる service をすべて読む
- 次を一覧にする:
  - 成功時の status / body / header (`cacheSec` などのキャッシュヘッダーを含む)
  - `meta.errors` の各エラーと、それを投げる条件
  - DB の読み書き (どのテーブルに、replica と primary のどちらで、どの順で)
  - Redis・イベント発行・キューなどの副作用と、その順序
  - 値の直列化 (`Date` の `toJSON`、`pack()` の出力形、undefined のプロパティの省略)
- 一覧を [behavior.csv](../../../../../docs/ts-to-rs/behavior.csv) に行として足す

## 2. ベースラインを取る

- 移す前の TS の実際の応答を記録し、フィクスチャにする (正常系・各エラー・境界値)
- 期待値を手で書かない

## 3. 実装する

- handler は専用の関数 (できれば専用モジュール) にし、dispatch 表に登録する。`execute` の `match` に本体を書かない
- 認証・認可・レート制限・バリデーションは既存の共通処理を通す。handler の中で重複して実装しない
- native endpoint の一覧は Rust 側の dispatch 表だけに書く。TS 側の一覧 (`executeLegacy` の拒否リストなど) を手で増やさない ([compatibility.md](../knowledge/compatibility.md) の 3 章)
- [rust-style.md](../knowledge/rust-style.md) に従う

## 4. 確かめる

- 2 のフィクスチャとの差分テストがすべて一致すること。一致しないケースは、承認済みの例外として記録されたものだけ
- `SKILL.md` の検証コマンドを実行する
- TS 側を変えたら ESLint と最も近い TS のテストも実行する

## 5. 記録する

- PORTS.md の endpoint 数など、契約の記述を更新する
- 検証結果を evidence ファイルに PASS / FAIL / BASELINE / SKIPPED で記録する
- [known-debt.md](../knowledge/known-debt.md) の項目を直したら、その行を消す
