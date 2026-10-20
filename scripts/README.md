# リポジトリ用スクリプト

コマンドはリポジトリのルートで実行します。入口はルートの [package.json](../package.json) です。

## 配置

```text
scripts/
├── build/                  ビルド用メタデータ、ソース配布物、生成物の削除
├── dev/                    開発用プロセスの起動と停止
├── checks/                 型、ライセンス、変更の完了前検査
├── benchmarks/
│   ├── backend/            HTTP・DB を含むバックエンド計測
│   └── storage/            ファイル I/O とランタイム API の計測
└── lib/                    スクリプト間で共有する処理
```

コマンド名の `:` は処理の分類に使います。`build:tarball:optional` は `build` → `tarball` → 条件付き実行です。
単語をつなぐ場合は `-` を使います（例: `generate:api-types`）。ファイル名も同じく `source-tarball.mjs` のように付けます。
スクリプトの実装は用途別ディレクトリへ置き、旧名への別名や転送用ファイルは追加しません。

## ビルドと開発

| コマンド | 実行ファイル | 役割 |
| --- | --- | --- |
| `bun run build:metadata` | [build/metadata.mjs](build/metadata.mjs) | バージョン情報を `built/meta.json` に生成する。`--watch` で変更を監視する |
| `bun run build:tarball` | [build/source-tarball.mjs](build/source-tarball.mjs) | ソース配布物を `built/tarball/` に生成する |
| `bun run build:tarball:optional` | [build/optional-tarball.mjs](build/optional-tarball.mjs) | 設定の `publishTarballInsteadOfProvideRepositoryUrl` が有効な場合にソース配布物を生成する |
| `bun run dev` | [dev/start.mjs](dev/start.mjs) | 初期ビルド後、各パッケージの watcher を起動し、終了時に停止する |
| `bun run clean` | [build/clean.mjs](build/clean.mjs) | ビルド生成物を削除する |
| `bun run clean:all` | [build/clean-all.mjs](build/clean-all.mjs) | ビルド生成物と依存関係の `node_modules` を削除する |

`bun run build` はメタデータ生成 → 各 workspace のビルド → 条件付きソース配布物生成の順に実行します。
`bun run build:storybook` は frontend の Storybook ビルドへ、`bun run generate:api-types` は backend API からの misskey-js 型再生成へ進みます。

## 検査

| コマンド | 実行ファイル | 役割 |
| --- | --- | --- |
| `bun run check:declarations` | [checks/declarations.mjs](checks/declarations.mjs) | リポジトリ管理下の手書き `.d.ts` を検査する |
| `bun run check:spdx` | [checks/spdx.mjs](checks/spdx.mjs) | SPDX ヘッダーを検査する。`--fix` でローカル変更の不足を補う |
| `bun run check:shipping` | [checks/shipping.mjs](checks/shipping.mjs) | 変更ファイルの lint、SPDX、locale safety をまとめて検査する |
| `bun run --filter frontend typecheck` | [checks/vue-typecheck.cjs](checks/vue-typecheck.cjs) | Vue テンプレートを含む型検査を実行する |

`vue-typecheck.cjs` は、Volar の compiler 変換を Bun 上で適用するための補助処理です。
frontend の設定を作業ディレクトリから読み込むため、上表の workspace コマンド経由で実行します。

## 計測とスクリプトのテスト

| コマンド | 実行ファイル | 役割 |
| --- | --- | --- |
| `bun run bench:backend` | [benchmarks/backend/controller.ts](benchmarks/backend/controller.ts) | 専用 PostgreSQL・Redis を使ってバックエンドを計測する。Docker が必要 |
| `bun run bench:storage` | [benchmarks/storage/run.mjs](benchmarks/storage/run.mjs) | ストレージとランタイム API を計測する |
| `bun run test:scripts` | `checks/` と `benchmarks/` の既存テスト | 型検査の補助、CLI、計測処理、子プロセスとの通信を検証する |

`bun run bench:backend --list` と `bun run bench:storage --list` で計測対象を列挙できます。
バックエンド計測の controller は [worker.ts](benchmarks/backend/worker.ts) を子プロセスで起動し、`registry.ts` に登録されたシナリオを実行します。
ストレージ計測は [files.mjs](benchmarks/storage/files.mjs) と [runtime.mjs](benchmarks/storage/runtime.mjs) がケースを定義します。

詳しい計測条件と結果の読み方は、[バックエンド計測](../fixtures/benchmark/README.md)と[ストレージ計測](benchmarks/storage/README.md)を参照してください。
