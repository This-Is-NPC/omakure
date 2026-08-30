# Fedora VM Privilege Certification

**Status:** Manual certification; static and cleanup subtests can run in CI-like environments, but the real VM path requires a Fedora/libvirt host.

## Source

- `.scripts/fedora-vm-privilege-certification.sh`
- `.scripts/fedora-vm-privilege-certification-static-test.sh`
- `.scripts/fedora-vm-privilege-certification-cleanup-test.sh`
- `.scripts/fixtures/fedora-vm-privilege/`

## Run

```bash
./.scripts/fedora-vm-privilege-certification-static-test.sh
timeout --foreground --kill-after=30s 30m ./.scripts/fedora-vm-privilege-certification.sh
timeout --foreground --kill-after=30s 25m ./.scripts/fedora-vm-privilege-certification-cleanup-test.sh
```

`mise run vm-privilege-certification` runs the static checks before the bounded
30-minute live certification.

## Proves

- The Fedora guest provisioning fixture, Polkit/service policy, Unix Battery fixture, and privilege boundaries contain the expected fixed operations.
- A real KVM/libvirt Fedora guest can exercise the privileged Battery operation through the intended service boundary.
- The live path verifies the node/service identity and reports safe operation results.
- Cleanup handles partial startup and induced failure without deleting resources it cannot positively identify.

## Does Not Prove

- It does not prove Windows service or macOS launchd installation; those paths remain separately documented limitations.
- Fedora cleanup inspection is fail-closed: inability to inspect a domain or volume is a failure, never permission to continue.
- A static test is not evidence that a real VM booted.

## Prerequisites, Bounds, Cleanup

Requires KVM/libvirt, Fedora guest tooling, sufficient host privileges, and the
fixture dependencies. The live run is bounded at 30 minutes and cleanup at 25
minutes, with a 30-second kill-after margin. Domain, volume, network, and
temporary artifacts are inspected by exact run identifiers; ambiguous matches
are preserved and reported.

## Troubleshooting

- Permission failure: inspect libvirt group/Polkit setup without broadening policy blindly.
- Cleanup cannot inspect: stop and fix the inspection permission/tooling; do not manually delete an ambiguous domain or volume.
