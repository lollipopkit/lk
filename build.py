import os
import sys

args = sys.argv
if len(args) < 2:
    print("Usage: python3 build.py <version>")
    exit(1)
version = args[1]

arch = ['arm64', 'amd64']
platform = ['darwin', 'linux', 'windows']

for a in arch:
    for p in platform:
        suffix = '.exe' if p == 'windows' else ''
        cmd = f'GOOS={p} GOARCH={a} go build -o releases/lk-{p}-{a}-v{version}{suffix}'
        code = os.system(cmd)
        if code != 0:
            print(f'Failed to build {p}-{a}')
            exit(code)
        print(f'Successfully built {p}-{a}')
