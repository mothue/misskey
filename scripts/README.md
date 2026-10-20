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
| `bun run dev-docker:setup` | [dev/docker.mjs](dev/docker.mjs) | 開発専用設定を生成し、Docker の PostgreSQL・Redis を起動してホスト側 Bun でビルドする |
| `bun run dev-docker:migrate` | [dev/docker.mjs](dev/docker.mjs) | 開発専用設定で migration を適用する |
| `bun run dev-docker:start` | [dev/docker.mjs](dev/docker.mjs) | 開発専用設定とストレージを使い、ホスト側 Bun で起動する |
| `bun run dev-docker:destroy` | [dev/docker.mjs](dev/docker.mjs) | 所有権を検査し、専用コンテナと PostgreSQL・Redis の全データを削除する |
| `bun run dev-docker:clean-cache` | [dev/docker.mjs](dev/docker.mjs) | 接続中 Docker の未使用ビルドキャッシュ全体を削除する |
| `bun run clean` | [build/clean.mjs](build/clean.mjs) | ビルド生成物を削除する |
| `bun run clean:all` | [build/clean-all.mjs](build/clean-all.mjs) | ビルド生成物と依存関係の `node_modules` を削除する |

`bun run build` はメタデータ生成 → 各 workspace のビルド → 条件付きソース配布物生成の順に実行します。
`bun run build:storybook` は frontend の Storybook ビルドへ、`bun run generate:api-types` は backend API からの misskey-js 型再生成へ進みます。

## Docker を使うローカル開発環境

ホスト側に Bun、Rust/Cargo、CMake、C/C++ コンパイラ、FFmpeg を用意し、Docker Engine と Docker Compose を起動しておきます。
Docker はローカルの Unix socket / named pipe に接続してください。`DOCKER_HOST` や Docker context が TCP / SSH 接続の場合は、ループバックの接続先との混同を避けるため拒否します。
PostgreSQL 18 と Redis 7 は公式イメージを取得して起動し、Misskey のフロントエンド・バックエンドはホスト側でビルド・実行します。

```bash
bun run dev-docker:setup
bun run dev-docker:migrate
bun run dev-docker:start
```

CLI のメッセージは英語で表示します。すべてのコマンドで実行前に `Continue? [y/N]` を確認します。`y` または `Y` だけが続行で、空入力・`n`・EOF・確認中の Ctrl+C は中止です。
`setup` は migration と本体起動を自動では行いません。初回の Rust ビルドには時間がかかります。

ブラウザで `http://localhost:3000` を開きます。開発環境では初期設定パスワードを設定しないため、その欄は空欄で進めてください。
PostgreSQL は `127.0.0.1:15432`、Redis は `127.0.0.1:16379` で待ち受けます。既に同じポートを使うコンテナがある場合は、先にその開発環境を停止してください。
外部サーバーとの連合受信を行うには、別途公開 HTTPS URL を使う環境が必要です。

設定・認証情報・生成済み JSON・アップロードファイルは Git 管理外の `.config/dev-docker/` に保存します。
既存の `.config/default.yml`、`.config/docker.env`、`built/.config.json`、`./db`、`./redis`、`./files` は上書き・削除しません。
生成済みの開発設定を作り直すときは、変更されていたファイルを `.config/dev-docker/backups/` へ退避します。DB パスワードは再実行でも維持します。
`migrate`・`start` は専用設定と専用 JSON 出力先を明示して実行するため、通常の `bun run migrate`・`bun run start` と区別できます。
両コマンドは、所有確認済みの DB・Redis が専用ポートで起動中であることも確認します。
`setup` のフロントエンドは配信用にビルドし、`start` で配信します。自動リロードは行いません。フロントエンドを変更したら `setup` を再実行してください。
アップロード先は `MISSKEY_FILES_DIR` で分離し、ビルド済みアセットの参照先はリポジトリ内のままにします。
生成された開発設定を変更した場合は `setup` を再実行してください。

DB と Redis を空にしてデバッグし直す場合は、ホスト側で動く Misskey を Ctrl+C で停止してから実行します。

```bash
bun run dev-docker:destroy
bun run dev-docker:setup
bun run dev-docker:migrate
bun run dev-docker:start
```

`destroy` の対象は、この checkout 用の `misskey-dev-<hash>` Compose プロジェクトだけです。
Docker daemon の ID、checkout の実パス、コンテナ・ボリューム・ネットワークの開発用ラベルと所有者 ID、コンテナのマウント先を検査します。
所有情報がない、別の Docker daemon に接続している、本番ラベルや bind mount が混ざっている場合は、削除開始前に拒否します。
`NODE_ENV=production` では全コマンドを拒否します。
開発設定・バックアップ・アップロードファイルは `destroy` 後も残します。`state.json` はリソースの所有確認に使うため、Docker リソースを削除する前に手で消さないでください。

操作中は `.config/dev-docker/operation.lock` で同時実行を防ぎます。`start` の実行中もロックするため、先に本体を停止してから `destroy` します。
強制終了などでロックが残った場合は、同じ checkout の操作が動いていないことを確認してから、空の `operation.lock` ディレクトリだけを削除してください。

```bash
bun run dev-docker:clean-cache
```

これは `docker builder prune --all` に相当し、接続中 Docker の**他プロジェクトを含む未使用ビルドキャッシュ**を削除します。
コンテナ・イメージ・DB・Redis のデータは削除しません。今回の構成では PostgreSQL・Redis のイメージを pull し、Misskey はホスト側でビルドするため、Misskey のネイティブビルド生成物を掃除する操作ではありません。

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
| `bun run test:scripts` | `checks/`・`benchmarks/`・`dev/` のテスト | 型検査の補助、CLI、計測処理、開発環境の分離と削除範囲、子プロセスとの通信を検証する |

実 Docker を使う開発環境スクリプトの検証は、次のコマンドで追加実行できます。

```bash
MISSKEY_DEV_DOCKER_INTEGRATION=1 bun test scripts/dev/docker.integration.test.mjs
```

一時 checkout とランダムポートで PostgreSQL・Redis の作成、データ書き込み、所有権の異なるリソースがある場合の拒否、削除後の空データでの再作成を検証します。
依存導入・ネイティブビルドはこのテストでは実行しません。通常の `test:scripts` では実 Docker を使うテストをスキップします。

`bun run bench:backend --list` と `bun run bench:storage --list` で計測対象を列挙できます。
バックエンド計測の controller は [worker.ts](benchmarks/backend/worker.ts) を子プロセスで起動し、`registry.ts` に登録されたシナリオを実行します。
ストレージ計測は [files.mjs](benchmarks/storage/files.mjs) と [runtime.mjs](benchmarks/storage/runtime.mjs) がケースを定義します。

詳しい計測条件と結果の読み方は、[バックエンド計測](../fixtures/benchmark/README.md)と[ストレージ計測](benchmarks/storage/README.md)を参照してください。
