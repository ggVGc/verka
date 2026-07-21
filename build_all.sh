#!/usr/bin/env bash

set -e

pushd ./linka && cargo build --release && popd
pushd ./orka-web && cargo build --release && popd
pushd ./driva && cargo build --release && popd
pushd ./orka && cargo build --release && popd
pushd ./nota && cargo build --release && popd
