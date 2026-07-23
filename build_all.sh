#!/usr/bin/env bash

set -e

(cd ./linka && cargo build --release)
(cd ./orka-web && cargo build --release)
(cd ./driva && cargo build --release)
(cd ./genta && cargo build --release)
(cd ./orka && cargo build --release)
(cd ./nota && cargo build --release)
(cd ./styra && cargo build --release)
