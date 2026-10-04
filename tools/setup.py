#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-only
"""Install build-only vendor dependencies at fixed revisions, with hash checks."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import urllib.request
from build_diag import SDK_SHA256

ROOT = Path(__file__).resolve().parents[1]
DEPS = ROOT / '.deps'
TC_VERSION = 'jieli-linux-toolchains-20250324.1'
TC_URL = 'https://jl-update.oss-cn-shenzhen.aliyuncs.com/' + TC_VERSION + '.tar.xz'
TC_SHA256 = 'f686586bcfb45e0f0bb27fd2b39c7a7f313cb4f0e88a66a14da621ffa8225958'
SDK_URL = 'https://gitee.com/Jieli-Tech/fw-AC79_AIoT_SDK.git'
SDK_TAG = 'AC79NN_SDK_V1.2.1_2023-12-13'
SDK_COMMIT = 'd179b4484759423312073f5fbb232501aa491047'

def digest(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()

def main():
    DEPS.mkdir(exist_ok=True)
    tc = Path(os.environ.get('JIELI_TOOLCHAIN', DEPS / 'toolchain'))
    sdk = Path(os.environ.get('AC79_SDK', DEPS / 'sdk'))
    archive = DEPS / (TC_VERSION + '.tar.xz')
    if not (tc / 'pi32v2/bin/clang').exists():
        if not archive.exists():
            print('Downloading pinned JieLi toolchain...', flush=True)
            tmp = archive.with_suffix('.part')
            with urllib.request.urlopen(TC_URL, timeout=120) as src, tmp.open('wb') as dst:
                shutil.copyfileobj(src, dst)
            tmp.replace(archive)
        if digest(archive) != TC_SHA256:
            raise SystemExit(f'Checksum mismatch: {archive}; remove it and retry.')
        with tarfile.open(archive) as tf:
            tf.extractall(DEPS, filter='data')
        tc.parent.mkdir(parents=True, exist_ok=True)
        if tc.is_symlink() and not tc.exists():
            tc.unlink()
        if tc != DEPS / TC_VERSION:
            tc.symlink_to(DEPS / TC_VERSION, target_is_directory=True)
    # Archive aliases can reset the shared clang inode's mode during filtered extraction.
    for rel in ('pi32v2/bin/clang', 'pi32v2/bin/ld', 'common/bin/objcopy', 'common/bin/objdump'):
        binary = tc / rel
        binary.chmod(binary.stat().st_mode | 0o111)
    # Record hash provenance even when reusing an installation. build-manifest also
    # records exact compiler bytes; a user-supplied installation is not silently repinned.
    if not sdk.exists():
        subprocess.run(['git', 'clone', '--depth', '1', '--filter=blob:none', '--sparse',
                        '--branch', SDK_TAG, SDK_URL, str(sdk)], check=True)
        subprocess.run(['git', '-C', str(sdk), 'sparse-checkout', 'set', 'cpu/wl82/tools'], check=True)
    commit = subprocess.check_output(['git', '-C', str(sdk), 'rev-parse', 'HEAD'], text=True).strip()
    if commit != SDK_COMMIT:
        raise SystemExit(f'SDK revision mismatch: {commit}; expected {SDK_COMMIT}')
    for rel, sha in SDK_SHA256.items():
        if digest(sdk / 'cpu/wl82/tools' / rel) != sha:
            raise SystemExit(f'SDK checksum mismatch: {rel}')
    print('Vendor dependencies ready. SDK revision and three packaging blobs verified.')

if __name__ == '__main__':
    main()
