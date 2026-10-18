## test-federation
Test federation between two Misskey servers: `a.test` and `b.test`.

Before testing, you need to build the entire project, and change working directory to here:
```sh
bun run build
cd packages/backend/test-federation
```

First, you need to start servers by executing following commands:
```sh
bash ./setup.sh
docker compose up --scale tester=0
```

The tester image includes Rust, Cargo, C/C++ build tools, and CMake to build the
local slacc addon. Compose builds it automatically on first use. After changing
the tester Dockerfile or toolchain versions, run `docker compose build tester`.

Then you can run all tests by a following command:
```sh
docker compose run --no-deps --rm tester
```

For testing a specific file, run a following command:
```sh
docker compose run --no-deps --rm tester bun run --filter backend test:fed test-federation/test/user.test.ts
```
