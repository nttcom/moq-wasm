#!/usr/bin/env bash

# Copyright (c) Meta Platforms, Inc. and affiliates.
# This source code is licensed under the MIT license found in the
# LICENSE file in the root directory of this source tree.

# Get base 64 of scert cmd
certbase64=$(eval "openssl x509 -pubkey -noout -in ./moqt-server-sample/keys/cert.pem | openssl rsa -pubin -outform der | openssl dgst -sha256 -binary | base64")

/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome --test-type --origin-to-force-quic-on=34.147.225.127:4434 --allow-insecure-localhost --ignore-certificate-errors --ignore-certificate-errors-spki-list=4n3yZTYnzUj1DEd6TwuMmuvtDAz92DPXcaup+VPUs58= --use-fake-device-for-media-stream 