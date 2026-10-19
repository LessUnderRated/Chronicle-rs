#!/usr/bin/env bash
# Publishes the release APK built here (./gradlew assembleRelease) as the GitHub release
# android-v<versionName> at the commit checked out, as .github/workflows/android.yml does from CI.
# Uses gh's REST calls only; GH_TOKEN needs Contents write on the repository. A version that has a
# release already is left alone, so CI's publish step and this one never both make one.
set -euo pipefail
cd "$(dirname "$0")/.."
repo=${GITHUB_REPOSITORY:-LessUnderRated/Chronicle-rs}
version=$(sed -n 's/^ *versionName = "\(.*\)"$/\1/p' android/app/build.gradle.kts)
tag="android-v${version}"
built=android/app/build/outputs/apk/release/app-release.apk
symbols_built=$(ls -t $(find android/app/build/intermediates/cxx -path '*arm64-v8a/libmain.so') | head -n 1)
if gh api "repos/$repo/releases/tags/$tag" > /dev/null 2>&1; then
    echo "$tag is already released"
    exit 0
fi
git fetch -q origin "$(git rev-parse --abbrev-ref HEAD)"
if [ "$(git rev-parse HEAD)" != "$(git rev-parse FETCH_HEAD)" ]; then
    echo "push the commit first: the release is made at it"
    exit 1
fi
out=$(mktemp -d)
apk="$out/Chronicle-${version}-arm64.apk"
symbols="$out/libmain-${version}-arm64-symbols.so"
cp "$built" "$apk"
cp "$symbols_built" "$symbols"
sha=$(sha256sum "$apk" | cut -d' ' -f1)
notes="Chronicle ${version} for Android: arm64 phones with Vulkan 1.3, Android 13 or later.

No game data is included: on first start the app asks for a disc image of your own Dark Cloud
disc and extracts it. See docs/ANDROID.md.

Signed with the development key in the repository (android/app/chronicle-dev.jks), so it installs
over the releases before it that were signed with it (0.5.0 on).

If it crashes, open it again: it offers the log of the run that crashed to share.
\`$(basename "$symbols")\` is for reading such a log; you don't need it to play.

SHA-256: \`${sha}\`"
id=$(gh api "repos/$repo/releases" -f tag_name="$tag" -f target_commitish="$(git rev-parse HEAD)" \
    -f name="Chronicle ${version} (Android)" -f body="$notes" --jq .id)
for file in "$apk" "$symbols"; do
    gh api -X POST "https://uploads.github.com/repos/$repo/releases/$id/assets?name=$(basename "$file")" \
        -H "Content-Type: application/octet-stream" --input "$file" --jq .browser_download_url
done
rm -rf "$out"
