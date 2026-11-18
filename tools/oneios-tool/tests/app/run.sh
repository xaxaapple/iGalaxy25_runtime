#!/bin/bash
# oneios-tool app with a stand-in OneOS checkout (its gradlew is a script), a stand-in gh and the stand-in phone:
# no JDK, no Android SDK, no phone and no network needed. The app is never built for real here.
#
#   tools/oneios-tool/tests/app/run.sh        one line per check, then `ALL OK` (exit 0) or `FAILED: N`
#
# What it checks: where the checkout is found and that the way is said (--oneos, ONEOS_DIR, the current folder;
# app/ of a stand-in OneiOS checkout, the submodule: present, empty with the command to run, with other files,
# from a worktree; OneOS beside the checkout; none found; `app dir`); version (the gradle fields, the series suffix, the commit
# and the dirty state, the installed app's version and certificate); build (the gradle command, --no-test, a
# failing build, the APK's facts); doctor (each prerequisite missing and present; a token is never printed);
# install (a new install, the same certificate, another certificate refused without an uninstall, a session
# open, --release through gh with its .sha256); stage-scripts (the three files, their sha256 before and after).
# `app version --no-phone` also runs on the real OneOS clone when there is one. Needs python3, openssl, git.
set -u
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT=$(cd "$HERE/../../../.." && pwd)
TOOL=${TOOL:-$ROOT/tools/oneios-tool/target/release/oneios-tool}
W=${WORK:-$ROOT/out/rustport-app}
[ -x "$TOOL" ] || { echo "run.sh: no $TOOL" >&2; exit 2; }
. "$HERE/../phone/lib.sh"
rm -rf "$W"; mkdir -p "$W/home" "$W/bin"
export ADB=$HERE/../phone/fake-adb.sh FAKE_ADB_LOG=$W/adb.log HOME=$W/home
unset ANDROID_SERIAL FAKE_NO_ROOT FAKE_DEVICES ONEOS_DIR ONEIOS_ROOT JAVA_HOME ANDROID_HOME ANDROID_SDK_ROOT GH_PACKAGES_USER GH_PACKAGES_TOKEN
REAL=$(HOME=~ fork_dir)
app() { "$TOOL" app "$@"; }

# ---- a stand-in checkout
F=$W/OneOS
mkdir -p "$F/app-android/app/src/full/assets/root" "$F/switcher/assets"
echo "// stand-in" > "$F/app-android/settings.gradle.kts"
cat > "$F/app-android/app/build.gradle.kts" <<'G'
android {
    compileSdk { version = release(37) }
    defaultConfig {
        applicationId = "com.fahadali.oneos"
        versionCode = 115
        versionName = "0.4.86"
    }
}
G
echo "# switch of the stand-in fork" > "$F/switcher/assets/oneios_switch.sh"
echo "# return of the stand-in fork" > "$F/switcher/assets/oneios_return.sh"
echo "# root script of the stand-in fork" > "$F/app-android/app/src/full/assets/root/oneios_root.sh"
# two keys, and an APK signed by each
for k in a b; do
  openssl req -x509 -newkey rsa:2048 -nodes -keyout "$W/$k.key" -out "$W/$k.der" -outform DER -days 2 -subj "/CN=stand-in $k" 2>/dev/null
  python3 -I "$HERE/mkapk.py" "$W/$k.apk" "$W/$k.der" "apk $k"
  printf 'Packages:\n  Package [com.fahadali.oneos] (1a2b3c4):\n    versionCode=117 minSdk=31 targetSdk=37\n    versionName=0.4.88\n' > "$W/$k.apk.package.txt"
done
certa=$(openssl dgst -sha256 "$W/a.der" | sed 's/.*= //'); certb=$(openssl dgst -sha256 "$W/b.der" | sed 's/.*= //')
cat > "$F/app-android/gradlew" <<G
#!/bin/sh
echo "\$PWD: gradlew \$*" >> "$W/gradle.log"
[ ! -e "$W/gradle-fails" ] || { echo "FAILURE: Build failed with an exception."; exit 1; }
mkdir -p app/build/outputs/apk/full/debug && cp "$W/a.apk" app/build/outputs/apk/full/debug/app-full-debug.apk
echo "BUILD SUCCESSFUL"
G
chmod +x "$F/app-android/gradlew"
(cd "$F" && git init -q -b oneios . && git add -A && git -c user.name=t -c user.email=t@example.invalid commit -q -m stand-in)

# ---- where the checkout is found
check "--oneos" 0 "^OneOS checkout: $F \(--oneos\)" app --oneos "$F" version --no-phone
check "ONEOS_DIR" 0 "^OneOS checkout: $F \(ONEOS_DIR\)" env ONEOS_DIR="$F" "$TOOL" app version --no-phone
check "the current folder" 0 "^OneOS checkout: $F \(the current folder\)" sh -c "cd '$F' && '$TOOL' app version --no-phone"
check "--oneos wins over ONEOS_DIR" 0 "\(--oneos\)" env ONEOS_DIR=/nowhere "$TOOL" app --oneos "$F" version --no-phone
check "--oneos of a folder that is no checkout" 1 "^oneios-tool app: --oneos: $W is not a OneOS checkout \(no app-android/app/build.gradle.kts\)" app --oneos "$W" version --no-phone
mkdir "$W/nothing"
check "--oneos of an empty folder" 1 "--oneos: $W/nothing is an empty folder, not a OneOS checkout \(if it is OneiOS's app/, the submodule is not initialised: git submodule update --init --remote app\)" app --oneos "$W/nothing" version --no-phone
# a stand-in OneiOS checkout (its own git checkout, with what `cycle` knows a OneiOS checkout by)
oneios() { mkdir -p "$1/scripts" "$1/oneos" "$1/tools" && touch "$1/scripts/bg.sh" "$1/oneos/interface.md" "$1/tools/x" && (cd "$1" && git init -q . && git add -A && git -c user.name=t -c user.email=t@example.invalid commit -q -m x); }
at() { local d=$1; shift; (cd "$d" && "$TOOL" app "$@"); }   # app ... run from the folder d
# a OneiOS checkout with a worktree and no app/, and OneOS beside the checkout: found from inside the worktree
oneios "$W/side/OneiOS" && cp -r "$F" "$W/side/OneOS"
(cd "$W/side/OneiOS" && git worktree add -q .claude/worktrees/agent-x 2>/dev/null)
check "beside the OneiOS checkout, from a worktree" 0 "^OneOS checkout: $W/side/OneOS \(beside the OneiOS checkout\)" at "$W/side/OneiOS/.claude/worktrees/agent-x" version --no-phone
at "$W/side/OneiOS" version --no-phone 2>&1 | grep -q "^note:" && bad "no app/ at all: a note was printed" || ok "no app/ at all (a checkout from before the submodule): nothing is said about it"
nowhere=$(mktemp -d /tmp/oneios-app-test.XXXXXX)   # outside every git checkout
check "no checkout anywhere" 1 "no OneOS checkout \(a folder with app-android/app/build.gradle.kts and switcher/assets/oneios_switch.sh\): the current folder is not inside a OneiOS checkout" at "$nowhere" version --no-phone

# ---- app/ of the OneiOS checkout: the submodule
O=$W/sub/OneiOS; oneios "$O"
sub="git submodule update --init --remote app"
check "none found: no app/, nothing beside" 1 "^oneios-tool app: no OneOS checkout .*: $O/app is not there; $W/sub/OneOS is not there\. Name one with --oneos DIR or ONEOS_DIR, or get the submodule \(in the OneiOS checkout: $sub\)$" at "$O" version --no-phone
mkdir "$O/app"
check "app/ empty: not a checkout, and the command is said" 1 "^oneios-tool app: no OneOS checkout .*: $O/app is empty: the app submodule is not initialised \(in $O: $sub\); $W/sub/OneOS is not there\." at "$O" version --no-phone
out=$(at "$O" dir 2>/dev/null); st=$?
[ $st = 1 ] && [ -z "$out" ] && ok "app/ empty: app dir prints no path, status 1" || bad "app/ empty: app dir" "status $st: $out"
check "app/ empty, --oneos" 0 "^OneOS checkout: $F \(--oneos\)" at "$O" --oneos "$F" version --no-phone
check "app/ empty, ONEOS_DIR" 0 "^OneOS checkout: $F \(ONEOS_DIR\)" env ONEOS_DIR="$F" sh -c "cd '$O' && '$TOOL' app version --no-phone"
env ONEOS_DIR="$F" sh -c "cd '$O' && '$TOOL' app version --no-phone" 2>&1 | grep -q "^note:" && bad "app/ empty, ONEOS_DIR: a note was printed" || ok "app/ empty, a checkout named by hand: nothing is said about app/"
cp -r "$F" "$W/sub/OneOS"
check "app/ empty, OneOS beside: the clone beside" 0 "^OneOS checkout: $W/sub/OneOS \(beside the OneiOS checkout\)" at "$O" version --no-phone
check "app/ empty, OneOS beside: the note with the command" 0 "^note: $O/app is empty: the app submodule is not initialised \(in $O: $sub\)$" at "$O" version --no-phone
at "$O" version --no-phone 2>/dev/null | grep -q "submodule" && bad "the note is on stdout" || ok "app/ empty, OneOS beside: the note is on stderr, not in the command's output"
[ "$(at "$O" dir 2>/dev/null)" = "$W/sub/OneOS" ] && ok "app dir: the path alone" || bad "app dir" "$(at "$O" dir 2>&1 | tr '\n' '|')"
# a folder named app that holds files, but not the fork's: told by what it holds
mkdir -p "$O/app/app-android/app" && cp "$F/app-android/app/build.gradle.kts" "$O/app/app-android/app/"
check "app/ without the fork's switch script (not the fork's branch): passed over, said" 0 "^note: $O/app is not the fork's checkout \(no switcher/assets/oneios_switch.sh; in $O: $sub\)$" at "$O" version --no-phone
rm -rf "$O/app/app-android" && echo x > "$O/app/README.md"
check "app/ with other files: passed over, said" 0 "^note: $O/app is not the fork's checkout \(no app-android/app/build.gradle.kts; " at "$O" version --no-phone
check "app/ with other files: the clone beside" 0 "^OneOS checkout: $W/sub/OneOS \(beside the OneiOS checkout\)" at "$O" version --no-phone
# the submodule, initialised
rm -rf "$O/app" && cp -r "$F" "$O/app"
check "app/ present: first, before the clone beside" 0 "^OneOS checkout: $O/app \(app/ of the OneiOS checkout\)$" at "$O" version --no-phone
at "$O" version --no-phone 2>&1 | grep -q "^note:" && bad "app/ present: a note was printed" || ok "app/ present: no note"
check "app/ present, from a folder inside the OneiOS checkout" 0 "^OneOS checkout: $O/app \(app/ of the OneiOS checkout\)$" at "$O/tools" version --no-phone
check "app/ present, ONEIOS_ROOT from outside" 0 "^OneOS checkout: $O/app \(app/ of the OneiOS checkout\)$" env ONEIOS_ROOT="$O" sh -c "cd '$nowhere' && '$TOOL' app version --no-phone"
[ "$(at "$O" dir 2>&1)" = "$O/app" ] && ok "app/ present: app dir is app/" || bad "app/ present: app dir" "$(at "$O" dir 2>&1 | tr '\n' '|')"
check "app/ present: --oneos still wins" 0 "^OneOS checkout: $F \(--oneos\)" at "$O" --oneos "$F" version --no-phone
check "app/ present: ONEOS_DIR still wins" 0 "^OneOS checkout: $F \(ONEOS_DIR\)" env ONEOS_DIR="$F" sh -c "cd '$O' && '$TOOL' app version --no-phone"
check "app/ present: ONEOS_DIR says that app/ is not used" 0 "^note: ONEOS_DIR is set, so $O/app is not used$" env ONEOS_DIR="$F" sh -c "cd '$O' && '$TOOL' app version --no-phone"
check "app/ present: ONEOS_DIR of a folder that is no checkout is an error, app/ is not taken instead" 1 "^oneios-tool app: ONEOS_DIR: $W/nothing is an empty folder" env ONEOS_DIR="$W/nothing" sh -c "cd '$O' && '$TOOL' app version --no-phone"
check "app/ present: the current folder, when it is a checkout, comes first" 0 "^OneOS checkout: $F \(the current folder\)" env ONEIOS_ROOT="$O" sh -c "cd '$F' && '$TOOL' app version --no-phone"
check "app/ present: app build builds it" 0 "^APK: $O/app/app-android/app/build/outputs/apk/full/debug/app-full-debug.apk$" at "$O" build --no-test
tail -n 1 "$W/gradle.log" | grep -q "^$O/app/app-android: gradlew assembleFullDebug$" && ok "app/ present: gradlew ran in app/app-android" || bad "app/ present: where gradlew ran" "$(tail -n 1 "$W/gradle.log")"
# a worktree of the OneiOS checkout: app/ is not tracked here, so the worktree has none; then an empty one, as a
# new worktree of a checkout with the submodule has
(cd "$O" && git worktree add -q .claude/worktrees/agent-y 2>/dev/null)
check "a worktree without app/: app/ of the main checkout" 0 "^OneOS checkout: $O/app \(app/ of the OneiOS main checkout\)$" at "$O/.claude/worktrees/agent-y" version --no-phone
mkdir "$O/.claude/worktrees/agent-y/app"
check "a worktree with an empty app/: app/ of the main checkout" 0 "^OneOS checkout: $O/app \(app/ of the OneiOS main checkout\)$" at "$O/.claude/worktrees/agent-y" version --no-phone
check "a worktree with an empty app/: the note" 0 "^note: $O/.claude/worktrees/agent-y/app is empty: the app submodule is not initialised \(in $O/.claude/worktrees/agent-y: $sub\)$" at "$O/.claude/worktrees/agent-y" version --no-phone
rmdir "$O/.claude/worktrees/agent-y/app" && cp -r "$F" "$O/.claude/worktrees/agent-y/app"
check "a worktree with its own app/: that one" 0 "^OneOS checkout: $O/.claude/worktrees/agent-y/app \(app/ of the OneiOS checkout\)$" at "$O/.claude/worktrees/agent-y" version --no-phone
# a real git submodule (the stand-in fork, branch oneios, as app/ of a stand-in OneiOS checkout): what a clone
# without --recurse-submodules and a new worktree leave in app/, and that the command the tool names fills it
RS=$W/real/src; RC=$W/real/OneiOS; gitf() { git -c protocol.file.allow=always -c user.name=t -c user.email=t@example.invalid "$@"; }
oneios "$RS"
if (cd "$RS" && gitf submodule add -q -b oneios "$F" app && gitf commit -q -m "the app submodule") > "$W/submodule.log" 2>&1 && git clone -q "$RS" "$RC" 2>> "$W/submodule.log"; then
  [ -d "$RC/app" ] && [ -z "$(ls -A "$RC/app")" ] && ok "a clone without its submodules: app/ is an empty folder" || bad "a clone without its submodules: app/ is not an empty folder" "$(ls -A "$RC/app" 2>&1 | tr '\n' ' ')"
  check "a clone without its submodules: not a checkout, the command is said" 1 "$RC/app is empty: the app submodule is not initialised \(in $RC: $sub\)" at "$RC" version --no-phone
  (cd "$RC" && gitf submodule update --init --remote app) >> "$W/submodule.log" 2>&1
  check "after the command the tool names: app/ is the checkout" 0 "^OneOS checkout: $RC/app \(app/ of the OneiOS checkout\)$" at "$RC" version --no-phone
  check "after the command the tool names: the submodule's version" 0 "^versionCode: 115$" at "$RC" version --no-phone
  (cd "$RC" && git worktree add -q .claude/worktrees/agent-z 2>> "$W/submodule.log")
  [ -d "$RC/.claude/worktrees/agent-z/app" ] && [ -z "$(ls -A "$RC/.claude/worktrees/agent-z/app")" ] && ok "a new worktree: its app/ is an empty folder" || bad "a new worktree: its app/ is not an empty folder"
  check "a new worktree: app/ of the main checkout, with the note" 0 "^note: $RC/.claude/worktrees/agent-z/app is empty: the app submodule is not initialised" at "$RC/.claude/worktrees/agent-z" version --no-phone
  check "a new worktree: app/ of the main checkout" 0 "^OneOS checkout: $RC/app \(app/ of the OneiOS main checkout\)$" at "$RC/.claude/worktrees/agent-z" version --no-phone
else echo "skipped: a real git submodule (git submodule add of a local checkout failed: $(tail -n 1 "$W/submodule.log"))"; fi
rmdir "$nowhere"
export ONEOS_DIR=$F

# ---- version
out=$(app version --no-phone 2>&1)
for want in "applicationId: com.fahadali.oneos \(the full flavour; probe: com.fahadali.oneos.probe\)" "versionName: 0.4.86 \(full: 0.4.86, probe: 0.4.86-probe\)" "versionCode: 115" "commit: [0-9a-f]{7,} \(branch oneios\), clean"; do
  printf '%s\n' "$out" | grep -Eq "^$want$" && ok "version: $want" || bad "version: $want" "$(printf '%s' "$out" | tr '\n' '|')"
done
echo "oneos.series=fold8" > "$F/app-android/gradle.properties"
check "version: the series suffix and a dirty tree" 0 "full: 0.4.86-fold8" app version --no-phone
check "version: dirty" 0 "dirty \(1 changed files\)" app version --no-phone
rm "$F/app-android/gradle.properties"
P=$W/phone; export FAKE_PHONE=$P; make_phone "$P"
mkdir -p "$P/data/app/~~fake==/com.fahadali.oneos-1=="; cp "$W/a.apk" "$P/data/app/~~fake==/com.fahadali.oneos-1==/base.apk"
check "version with a phone: the installed version" 0 "^installed: 0.4.86 \(versionCode 115\) on FAKE0001$" app version
check "version with a phone: the installed certificate" 0 "^installed certificate sha256: $certa$" app version
check "version with no phone attached" 0 "^installed: not asked \(no phone on adb" env FAKE_DEVICES= "$TOOL" app version
if [ -n "$REAL" ]; then
  check "version --no-phone on the real clone ($REAL)" 0 "^versionCode: [0-9]+$" env ONEOS_DIR="$REAL" "$TOOL" app version --no-phone
  env ONEOS_DIR="$REAL" "$TOOL" app version --no-phone | sed 's/^/    /'
else echo "skipped: version on the real clone (none found)"; fi

# ---- build
check "build" 0 "^APK: $F/app-android/app/build/outputs/apk/full/debug/app-full-debug.apk$" app build
tail -n 1 "$W/gradle.log" | grep -q "^$F/app-android: gradlew test assembleFullDebug$" && ok "build: cd app-android && ./gradlew test assembleFullDebug" || bad "build: the command" "$(tail -n 1 "$W/gradle.log")"
out=$(app build --no-test 2>&1)
tail -n 1 "$W/gradle.log" | grep -q ": gradlew assembleFullDebug$" && ok "build --no-test: assembleFullDebug only" || bad "build --no-test" "$(tail -n 1 "$W/gradle.log")"
for want in "size: $(stat -c %s "$W/a.apk")" "sha256: $(sha256sum < "$W/a.apk" | cut -d' ' -f1)" "version: 0.4.86 \(versionCode 115\)" "certificate sha256: $certa"; do
  printf '%s\n' "$out" | grep -Eq "^$want$" && ok "build: $want" || bad "build: $want" "$(printf '%s' "$out" | tr '\n' '|')"
done
touch "$W/gradle-fails"
check "build: a failing gradle is a failure" 1 "gradlew ended with exit status: 1" app build
rm "$W/gradle-fails"

# ---- doctor
check "doctor: nothing there" 1 "MISSING JDK 21|MISSING Android SDK" app doctor
out=$(app doctor 2>&1)
for want in "ok      gradlew" "MISSING Android SDK: none found" "MISSING GitHub Packages user" "MISSING GitHub Packages token"; do
  printf '%s\n' "$out" | grep -q "^$want" && ok "doctor, bare: $want" || bad "doctor, bare: $want" "$(printf '%s' "$out" | tr '\n' '|')"
done
mkdir -p "$W/jdk21/bin" "$W/jdk17/bin" "$W/sdk/platforms/android-37.0" "$W/sdk36/platforms/android-36" "$W/home/.config/twidget"
printf '#!/bin/sh\necho "openjdk version \\"21.0.5\\" 2025-10-15" >&2\n' > "$W/jdk21/bin/java"
printf '#!/bin/sh\necho "openjdk version \\"17.0.19\\" 2026-04-21" >&2\n' > "$W/jdk17/bin/java"
chmod +x "$W"/jdk*/bin/java
secret=ghp_STANDINSECRETSTANDINSECRET0123456789
printf 'ghUsername=someone\nghAccessToken=%s\n' "$secret" > "$W/home/.config/twidget/github.properties"
check "doctor: everything there" 0 "the build's prerequisites are there" env JAVA_HOME="$W/jdk21" ANDROID_HOME="$W/sdk" "$TOOL" app doctor
out=$(env JAVA_HOME="$W/jdk21" ANDROID_HOME="$W/sdk" "$TOOL" app doctor 2>&1)
for want in "ok      JDK 21: .* is 21.0.5" "ok      Android SDK: $W/sdk \(ANDROID_HOME\)" "ok      platform 37: platforms/android-37.0" "ok      GitHub Packages user: set \(github.properties\)" "ok      GitHub Packages token: set \(github.properties; a classic token, 40 characters"; do
  printf '%s\n' "$out" | grep -Eq "^$want" && ok "doctor: $want" || bad "doctor: $want" "$(printf '%s' "$out" | tr '\n' '|')"
done
printf '%s\n' "$out" | grep -q "STANDINSECRET\|someone" && bad "doctor printed the token or the user" || ok "doctor: neither the token nor the user name is printed"
check "doctor: JDK 17" 1 "MISSING JDK 21: .* is 17.0.19; the build's toolchain is 21" env JAVA_HOME="$W/jdk17" ANDROID_HOME="$W/sdk" "$TOOL" app doctor
check "doctor: an SDK without platform 37" 1 "MISSING platform 37: not in the SDK" env JAVA_HOME="$W/jdk21" ANDROID_SDK_ROOT="$W/sdk36" "$TOOL" app doctor
echo "sdk.dir=$W/sdk" > "$F/app-android/local.properties"
check "doctor: the SDK of local.properties" 0 "Android SDK: $W/sdk \(local.properties sdk.dir\)" env JAVA_HOME="$W/jdk21" "$TOOL" app doctor
rm "$W/home/.config/twidget/github.properties"
check "doctor: the login from the environment" 0 "GitHub Packages token: set \(GH_PACKAGES_TOKEN" env JAVA_HOME="$W/jdk21" GH_PACKAGES_USER=someone GH_PACKAGES_TOKEN=$secret "$TOOL" app doctor
out=$(env JAVA_HOME="$W/jdk21" GH_PACKAGES_USER=someone GH_PACKAGES_TOKEN=github_pat_STANDINSECRET "$TOOL" app doctor 2>&1); st=$?
[ $st = 1 ] && printf '%s\n' "$out" | grep -q "MISSING GitHub Packages token: set (GH_PACKAGES_TOKEN), but it is a fine-grained token" && ! printf '%s\n' "$out" | grep -q STANDINSECRET \
  && ok "doctor: a fine-grained token is refused, not printed" || bad "doctor: a fine-grained token" "$st"

# ---- install
rm -rf "$P/data/app" "$P/.fake/package.txt"
cp "$W/a.apk.package.txt" "$P/.fake/next-package.txt"   # what the phone says after an install
installed() { cmp -s "$1" "$P/data/app/~~fake==/com.fahadali.oneos-1==/base.apk"; }
check "install: nothing installed yet" 0 "^installed now: 0.4.88 \(versionCode 117\)$" app install
installed "$W/a.apk" && ok "install: the built APK is on the phone" || bad "install: the APK on the phone"
grep -q "^-s FAKE0001 install -r $F/app-android/app/build/outputs/apk/full/debug/app-full-debug.apk$" "$W/adb.log" && ok "install: adb install -r" || bad "install: the adb command" "$(grep install "$W/adb.log" | tail -n 1)"
check "install: the same certificate goes over the installed app" 0 "^installed: 0.4.88 \(versionCode 117\), certificate sha256 $certa$" app install "$W/a.apk"
check "install: another certificate is refused" 1 "refused: the installed app is signed with another certificate.*Do not uninstall: that wipes the app's data" app install "$W/b.apk"
installed "$W/a.apk" && ok "install refused: the installed app is untouched" || bad "install refused: the installed app changed"
grep -qi "uninstall" "$W/adb.log" && bad "an uninstall was sent to the phone" || ok "install: no uninstall is ever sent"
check "install: a file that is no APK" 1 "not an APK|signing block|no signing certificate" app install "$W/a.der"
check "install: no root" 1 "no root on the phone" env FAKE_NO_ROOT=1 "$TOOL" app install "$W/a.apk"
"$TOOL" phone switch --version-code 117 > /dev/null 2>&1; wait_for 5 has_session "$P"
check "install: refused while a session is open" 1 "a session is open: return to One UI first" app install "$W/a.apk"
check "stage-scripts: refused while a session is open" 1 "a session is open" app stage-scripts
"$TOOL" phone return > /dev/null 2>&1; wait_for 10 no_session "$P" || end_session "$P"
# --release: a stand-in gh that serves the prerelease's two files
cat > "$W/bin/gh" <<G
#!/bin/sh
echo "gh \$*" >> "$W/gh.log"
[ "\$1 \$2 \$3" = "release download oneios-0.4.88" ] || { echo "release not found" >&2; exit 1; }
while [ \$# -gt 1 ]; do shift; done
cp "$W/a.apk" "\$1/OneOS-oneios-0.4.88-debug.apk"
[ -e "$W/gh-bad-sum" ] && echo "0000  x" > "\$1/OneOS-oneios-0.4.88-debug.apk.sha256" || (cd "\$1" && sha256sum OneOS-oneios-0.4.88-debug.apk > OneOS-oneios-0.4.88-debug.apk.sha256)
G
chmod +x "$W/bin/gh"; export GH=$W/bin/gh
check "install --release" 0 "^sha256 as the release's .sha256: $(sha256sum < "$W/a.apk" | cut -d' ' -f1)$" app install --release 0.4.88 --dir "$W/dl"
grep -q "^gh release download oneios-0.4.88 --repo akshaynexus/OneOS --pattern OneOS-oneios-0.4.88-debug.apk " "$W/gh.log" && ok "install --release: gh release download oneios-<version>" || bad "install --release: the gh command" "$(tail -n 1 "$W/gh.log")"
check "install --release of a version that is not there" 1 "gh release download oneios-9.9.9 ended with" app install --release 9.9.9 --dir "$W/dl"
touch "$W/gh-bad-sum"
check "install --release: a download that is not the release's sha256" 1 "the release's .sha256 says 0000" app install --release 0.4.88 --dir "$W/dl"
rm "$W/gh-bad-sum"

# ---- stage-scripts
S=$P/data/local/tmp/oneios
link=$(readlink "$S/bin/oneios_root.sh"); before=$(sha256sum < "$link")
out=$(app stage-scripts 2>&1); st=$?
[ $st = 0 ] && ok "stage-scripts" || bad "stage-scripts" "$(printf '%s' "$out" | tail -n 3 | tr '\n' '|')"
for f in "switcher/assets/oneios_switch.sh oneios_switch.sh" "switcher/assets/oneios_return.sh oneios_return.sh" "app-android/app/src/full/assets/root/oneios_root.sh bin/oneios_root.sh"; do
  set -- $f; sum=$(sha256sum < "$F/$1" | cut -d' ' -f1)
  cmp -s "$F/$1" "$S/$2" && [ ! -L "$S/$2" ] && printf '%s\n' "$out" | grep -Eq "^$2: before [0-9a-f]{64}, after $sum \(replaced\)$" && ok "stage-scripts: $2 is the fork's, sha256 before and after" || bad "stage-scripts: $2" "$(printf '%s' "$out" | tr '\n' '|')"
done
[ "$(sha256sum < "$link")" = "$before" ] && ok "stage-scripts: what was there was replaced, not written through (a link's target is untouched)" || bad "stage-scripts wrote through a link"
[ "$(stat -c %a "$S/oneios_switch.sh")" = 700 ] && [ ! -e "$P/data/local/tmp/oneios-scripts-tmp" ] && ok "stage-scripts: mode 0700, the pushed copies are gone" || bad "stage-scripts: modes or leftovers"
check "stage-scripts again: unchanged" 0 "^bin/oneios_root.sh: before ([0-9a-f]{64}), after \1 \(unchanged\)$" app stage-scripts
if grep -Eqi 'find |ls -R|du |/sys/power|reboot|uninstall' "$W/adb.log"; then bad "a tree walk, /sys/power, a restart or an uninstall was sent to the phone"
else ok "nothing sent to the phone walks a tree, writes /sys/power, restarts or uninstalls ($(wc -l < "$W/adb.log") adb calls)"; fi

end_session "$P"
[ -n "${KEEP:-}" ] || rm -rf "$W"
[ $FAILED = 0 ] && echo "ALL OK" || { echo "FAILED: $FAILED"; exit 1; }
