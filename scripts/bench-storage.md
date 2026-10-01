# Storage microbenchmarks

```sh
bun run bench:storage --list
bun run bench:storage storage/buffer/4KiB
bun run bench:storage storage/copy --iterations 100 --warmup 10 --output /tmp/storage.json
bun run bench:storage --directory /path/to/filesystem
```

フィルタは部分一致、複数指定はORです。既定は全storageケース・warmup5回・測定30回。
決定的な4KiB/1MiB/16MiB bufferを準備し、同期fs・非同期fs・Bun.writeを同じBunで比較します。
計測後に内容を検証し、一時ファイルを片付けます。JSONには生sample・median/p95/mean・ops/sと実行環境を保存します。
fsyncは行わず、OS cacheやfilesystemのcopy-on-writeは有効です。永続化性能や並行処理性能の測定ではありません。
HTTP/DBを含むbackendベンチは`bun run bench`を使います。

`runtime/`フィルタでhashing・UUID・export writerの旧実装とBun実装も比較できます。

```sh
bun run bench:storage runtime --iterations 100 --warmup 20 --output /tmp/runtime.json
```

writerは双方ともWeb WritableStream経由で64個の16KiB chunkを追記し、完了まで待ちます。hashingは決定的なメモリ上のbuffer、UUIDは1 sampleあたり1,000回です。検証は計測後に実行します。小さな処理はtimer/JITの影響を受けるため単回の差を性能保証とは扱いません。
