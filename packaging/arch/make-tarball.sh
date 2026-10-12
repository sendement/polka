#!/bin/sh
# Snapshot the committed tree as the PKGBUILD's source tarball.
set -e
cd "$(dirname "$0")/../.."
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
git archive --format=tar.gz --prefix="polka-$ver/" -o "packaging/arch/polka-$ver.tar.gz" HEAD
echo "packaging/arch/polka-$ver.tar.gz"
