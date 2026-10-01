# Storage microbenchmarks

```sh
bun run bench:storage --list
bun run bench:storage storage/buffer/4KiB
bun run bench:storage storage/copy --iterations 100 --warmup 10 --output /tmp/storage.json
bun run bench:storage --directory /path/to/filesystem
```

フィルタは部分一致、複数指定はORです。既定は全storageケース・warmup5回・測定30回。
決定的な4KiB/1MiB/16MiB bufferを準備し、同期fs操作とBun.writeを同じBunで比較します。
計測後に内容を検証し、一時ファイルを片付けます。JSONには生sample・median/p95/mean・ops/sと実行環境を保存します。
fsyncは行わず、OS cacheやfilesystemのcopy-on-writeは有効です。永続化性能や並行処理性能の測定ではありません。
HTTP/DBを含むbackendベンチは`bun run bench`を使います。
