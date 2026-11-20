#!/usr/bin/env bash
# Linux half of the MatixBridge brand. Run after apply.sh, in the Linux build only.
#
# apply.sh sets APP_NAME to "MatixBridge", and the Linux code derives every name it uses at run
# time from the lowercased app name: `systemctl enable matixbridge`, `pkill -f "matixbridge --tray"`,
# `matixbridge --tray` through PATH, ~/.config/autostart/matixbridge.desktop
# (src/platform/linux.rs, src/core_main.rs). Upstream's deb ships all of those as "rustdesk", so
# without this script the package installs but its service and tray are never found.
#
# The deb FILE keeps upstream's rustdesk-<version>.deb name on purpose: the workflow's globs and
# matix-native.yml's rename step (rustdesk-* -> MatixBridge-*) work on that name. What changes is
# what the package installs: package `matixbridge`, /usr/share/matixbridge/matixbridge,
# /usr/bin/matixbridge, matixbridge.service, matixbridge.desktop, the Matix icons.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$root"

sub() { # file, perl substitution; fails if nothing matched
  local file="$1" expr="$2"
  perl -0pi -e "\$n += ($expr); END { exit(\$n ? 0 : 3) }" "$file" || { echo "brand-linux: no match in $file for $expr"; exit 1; }
}

# The executable (the core library stays lib/librustdesk.so, which main.cc loads by that name).
sub flutter/linux/CMakeLists.txt 's/set\(BINARY_NAME "rustdesk"\)/set(BINARY_NAME "matixbridge")/'

# What the deb installs, and where (build.py build_flutter_deb).
sub build.py 's#tmpdeb/usr/share/rustdesk#tmpdeb/usr/share/matixbridge#g'
sub build.py 's#tmpdeb/usr/bin/rustdesk#tmpdeb/usr/bin/matixbridge#g'
sub build.py "s#(cp \.\./res/rustdesk\.service tmpdeb/usr/share/matixbridge/files/systemd/)'#\$1matixbridge.service'#g"
sub build.py 's#apps/rustdesk\.(png|svg)#apps/matixbridge.$1#g'
sub build.py 's#applications/rustdesk\.desktop#applications/matixbridge.desktop#g'
sub build.py 's#applications/rustdesk-link\.desktop#applications/matixbridge-link.desktop#g'
sub build.py 's#"""Package: rustdesk\n#"""Package: matixbridge\n#'
sub build.py 's#Maintainer: rustdesk <info\@rustdesk\.com>#Maintainer: Sismatix Information Systems <contact\@sismatix.com>#'
sub build.py 's#Homepage: https://rustdesk\.com#Homepage: https://matixerp.com/matixbridge/#'
sub build.py 's#Description: A remote control software\.#Description: MatixBridge remote support (based on RustDesk, AGPL-3.0).#'

# Package scripts: symlink, service install/start/stop, purge.
for f in preinst postinst prerm postrm; do
  sub "res/DEBIAN/$f" 's/rustdesk/matixbridge/g'
done

# The root service (copied into place as matixbridge.service by postinst).
sub res/rustdesk.service 's/Description=RustDesk/Description=MatixBridge/'
sub res/rustdesk.service 's/rustdesk/matixbridge/g'

# Menu entries: name, command, icon and window class all follow the executable name.
sub res/rustdesk.desktop 's/^Name=RustDesk$/Name=MatixBridge/m'
sub res/rustdesk.desktop 's/^GenericName=.*$/GenericName=Remote Support/m'
sub res/rustdesk.desktop 's/^Comment=.*$/Comment=Matix remote support/m'
sub res/rustdesk.desktop 's/rustdesk/matixbridge/g'
sub res/rustdesk-link.desktop 's/^Name=RustDesk$/Name=MatixBridge/m'
sub res/rustdesk-link.desktop 's/rustdesk/matixbridge/g'

# Icons: the PNGs in res/ come from apply.sh; the scalable one is not in its asset set.
cp matix/brand/assets/flutter/assets/icon.svg res/scalable.svg

echo "brand-linux: package matixbridge, /usr/bin/matixbridge, matixbridge.service"
