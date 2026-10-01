# Misskey backend benchmark v1

リポジトリのルートから実行します。Bun、インストール済み依存関係、起動済みのDockerが必要です。

```sh
bun run bench                       # small、全5シナリオ
bun run bench --list                 # Docker・ビルド・DB接続なし
bun run bench timeline              # 名前の部分一致、複数フィルタはOR
bun run bench timeline/user --requests 10000 --concurrency 32 --warmup 1000
bun run bench --fixture standard --json /tmp/benchmark.json
bun run bench:test
```

既定値はsmall、測定1,000件、warmup100件、concurrency8、timeout10,000msです。
件数はシナリオ全体で配分され、正の整数が必要です。`--timeout`の単位はmsです。
シナリオは`timeline/global`、`timeline/home`、`timeline/user`、`reaction/list`、`note/create`の順です。
読み取りlimit20、投稿本文256文字・public・localOnlyを使います。

## 専用環境

controllerが専用compose project `misskey-benchmark-v1`のPostgreSQL・Redisを起動し、
必要な共有パッケージとbackend専用entryをビルドします。API61912、PostgreSQL54412、Redis56412を使い、
既存プロセスは停止しません。実行ロックで同時実行を拒否します。

通常のbackendビルドや本番設定は書き換えません。専用runtimeは`.benchmark/config.json`を読み、
DB名・接続先・専用storage rootを起動時に検査します。既存migrationを専用作業領域へコピーして実行します。
元のmigrationは変更しません。HTTP worker1個と通常のqueue processorを同じサーバープロセスで起動します。
`NODE_ENV=benchmark`により既存レート制限を無効化し、SQL検索経路を選択します。
定期daemonは停止します。専用runtimeで外向きHTTP transportを拒否し、HTTP agentのproxyも閉じたloopbackへ向けます。リモートユーザー、連合投稿、メール設定、Webhookをfixtureに含めません。
外部配送・メール・Webhook・ファイル添付・検索サービスの性能はこの結果の対象外です。

終了時はcontroller所有のサーバー・generatorを停止し、DBコンテナを残します。
Ctrl+Cは新規送信を止め、取得済み結果をpartialとして保存し、10秒後に残った所有プロセスを終了します。

```sh
# コンテナを明示的に停止する場合
docker compose -p misskey-benchmark-v1 -f fixtures/benchmark/compose.yml down
```

## Fixture

pureな定義は`packages/backend/src/benchmark/definition.ts`、公開用の再exportは
`fixtures/benchmark/definition.ts`です。backend側のdecoratorを含む生成・起動コードはrolldownでコンパイルします。

| 項目 | small | standard |
| --- | ---: | ---: |
| users | 128 | 1,024 |
| notes | 8,192 | 131,072 |
| 通常ユーザーのfollow | 8 | 32 |
| 多followユーザーのfollow | 32 | 128 |
| リアクション対象ノート | 16 | 64 |
| 対象ごとのreaction | 32 | 256 |

version1、seed `misskey-benchmark-v1`、基準時刻`2026-01-01T00:00:00Z`を固定します。
75%が通常ユーザー、25%が多followユーザーで、各ユーザーに重複しないmute2件・block1件を割り当てます。
visibilityと本文長は固定順に割り当て、投稿者を固定PRNGで偏らせます。
端数を含む実際の分布をmanifestと結果へ保存します。規模は実データ由来ではなく仮設定です。

manifestの`users[].key`からユーザーID、`noteIds[index]`から論理キー`note/<index>`のIDが引けます。
fingerprintはversion・seed・規模・ユーザー・関係・ノート内容のSHA-256です。
fixture変更時はversionも更新してください。認証情報を含むmanifest・DB/Redis snapshot・設定は
ignoredな`.benchmark/`に保存します。結果JSONにはtokenやユーザーmanifestを含めません。

初期DB dumpとRedis dumpを保存し、各シナリオ前にDB・Redis・空のローカルstorageを復元します。
件数検査と全ユーザーをローテーションする読み取りAPI preflightで認証・内容を検証します。
投稿はwarmupで認証・本文を検証し、ノートは残します。queueのwaiting/active/prioritizedを最大60秒待ち、
開始・終了・終了5秒後のqueueと開始・終了のノート件数を記録します。
定期ジョブのdelayedや履歴completed/failedは消化待ちの対象にしません。

## 計測と結果

1シナリオにつき別Bun generator1プロセスを起動します。workerはclosed modelの非同期ループです。
HTTP本文を全消費するまでをlatencyとし、その後JSON解析・validationを実施します。
validation時間はlatencyには含めず、次の送信までの時間とthroughputには含めます。retryはしません。
warmupは測定から除外します。成功sampleはFloat64Arrayに保持し、IPCでcontrollerへ送ります。
終了時はcontrollerの受領確認を待ち、大きな結果の欠落を防ぎます。

schemaVersion1のJSONを毎回`.benchmark/results/`に自動保存し、`--json`の指定先にも保存します。
Git commit/dirty、Bun・PostgreSQL・Redis、OS/CPU、fixture fingerprint/分布、負荷条件、
設定の主要条件、generator CPU（user/system、microseconds）、生sampleを含みます。
success/HTTP error/timeout/transport error/validation failureを排他的に数え、失敗時間も別に保存します。
成功sample全体を使ってnearest-rankのp50/p90/p95/p99・min/maxを算出します。
成功throughputと完了throughputはいずれも測定フェーズのwall timeで割ります。

失敗・不完全・skippedがあればexit1です。シナリオ失敗時は後続を続け、fixture復元に失敗したら後続をskippedにします。
強制終了では最後に取得したprogressまでしか保存できない場合があります。
`--save/--compare`、複数run、open model、cold/warm、サーバーprofile、SQL自動収集は未実装です。

ストレージmicrobenchmarkは`bun run bench:storage`へ分離しています。詳細は`scripts/bench-storage.md`を参照してください。
