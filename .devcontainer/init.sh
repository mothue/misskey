#!/bin/bash

set -xe

sudo chown node node_modules
sudo apt-get update
sudo apt-get -y install build-essential cmake libgtk2.0-0 libgtk-3-0 libgbm-dev libnotify-dev libnss3 libxss1 libasound2 libxtst6 xauth xvfb
git config --global --add safe.directory /workspace
git submodule update --init
bun install --frozen-lockfile
cp .devcontainer/devcontainer.yml .config/default.yml
bun run build
bun run migrate
(cd packages/frontend && bun x --bun --no-install playwright install --with-deps chromium)
