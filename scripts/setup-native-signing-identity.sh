#!/usr/bin/env bash
# One-time setup of the stable self-signed code-signing identity used to sign
# Monoptah Native macOS releases (see docs/native/release-flow.md, "Stable signing identity").
#
#   scripts/setup-native-signing-identity.sh              create, import, write a backup .p12
#   scripts/setup-native-signing-identity.sh --import F   restore from backup file F (new Mac)
#
# Options:
#   --keychain PATH   keychain to use (default: the login keychain; env MONOPTAH_NATIVE_SIGNING_KEYCHAIN)
#   --backup PATH     where to write the backup (default: ~/monoptah-native-signing.p12)
#   --days N          certificate validity in days (default: 7300, about 20 years)
#
# The backup passphrase is typed interactively (read -s) or, for tests only, taken
# from the MONOPTAH_SIGNING_PASSPHRASE env var. It never appears on a command line:
# openssl reads it through `-passout/-passin env:`. The key and certificate are
# imported into the keychain as unencrypted PEM files from a 0700 temp dir that is
# removed on exit (`security import` has no stdin or env form for a passphrase).
# After the import it sets the key partition list so codesign never asks for
# "Always Allow"; that prompts for the keychain password (for a test keychain set
# MONOPTAH_KEYCHAIN_PASSWORD, which is then visible in argv, so never use it for the login keychain).
# Trust is NOT needed: codesign signs with an untrusted self-signed certificate.
set -euo pipefail

CN="Monoptah Native Release Signing"
mode=create
import_file=""
keychain="${MONOPTAH_NATIVE_SIGNING_KEYCHAIN:-}"
backup="$HOME/monoptah-native-signing.p12"
days=7300

usage() { sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; }
while [ $# -gt 0 ]; do
  case "$1" in
    --import) mode=import; import_file="${2:?--import needs a file}"; shift 2 ;;
    --keychain) keychain="${2:?--keychain needs a path}"; shift 2 ;;
    --backup) backup="${2:?--backup needs a path}"; shift 2 ;;
    --days) days="${2:?--days needs a number}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ "$(uname -s)" != Darwin ]; then
  echo 'This runs on macOS only.' >&2
  exit 1
fi
case "$days" in ''|*[!0-9]*) echo "--days must be a number" >&2; exit 2 ;; esac

keychain_args=()
if [ -n "$keychain" ]; then keychain_args=("$keychain"); fi

list_identities() { security find-identity -p codesigning ${keychain_args[@]+"${keychain_args[@]}"} 2>/dev/null || true; }
existing_sha1() { list_identities | sed -n "s/^ *[0-9]*) \([0-9A-F]\{40\}\) \"$CN\".*/\1/p" | head -n 1; }

found="$(existing_sha1)"
if [ -n "$found" ]; then
  echo "A signing identity named \"$CN\" already exists. Nothing was changed."
  echo "SHA-1: $found"
  echo "Use it with: export MONOPTAH_NATIVE_SIGNING_IDENTITY=$found"
  exit 0
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/monoptah-signing.XXXXXX")"
trap 'rm -rf "$work"' EXIT
unset MONOPTAH_P12_PASS

read_passphrase() {
  if [ -n "${MONOPTAH_SIGNING_PASSPHRASE:-}" ]; then
    MONOPTAH_P12_PASS="$MONOPTAH_SIGNING_PASSPHRASE"
  else
    local again
    printf 'Backup passphrase: ' >&2; IFS= read -r -s MONOPTAH_P12_PASS; printf '\n' >&2
    if [ "$mode" = create ]; then
      printf 'Repeat passphrase: ' >&2; IFS= read -r -s again; printf '\n' >&2
      [ "$MONOPTAH_P12_PASS" = "$again" ] || { echo 'Passphrases differ.' >&2; exit 1; }
    fi
  fi
  [ -n "$MONOPTAH_P12_PASS" ] || { echo 'The passphrase must not be empty.' >&2; exit 1; }
  export MONOPTAH_P12_PASS
}

# Imports an unencrypted PEM key (traditional RSA form, the one `security import`
# reads) and the certificate, both usable by codesign, then fixes the partition list.
import_pem() {
  local key="$1" cert="$2" kc=()
  if [ -n "$keychain" ]; then kc=(-k "$keychain"); fi
  security import "$key" -f openssl -T /usr/bin/codesign ${kc[@]+"${kc[@]}"} >/dev/null
  security import "$cert" -f pemseq -T /usr/bin/codesign ${kc[@]+"${kc[@]}"} >/dev/null
  [ -n "$(existing_sha1)" ] || return 0
  if [ -n "${MONOPTAH_KEYCHAIN_PASSWORD:-}" ]; then
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$MONOPTAH_KEYCHAIN_PASSWORD" \
      ${keychain_args[@]+"${keychain_args[@]}"} >/dev/null \
      || echo 'Warning: could not set the key partition list; codesign may ask for "Always Allow".' >&2
  else
    echo 'Setting the key partition list (asks for the keychain password) ...' >&2
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s \
      ${keychain_args[@]+"${keychain_args[@]}"} >/dev/null \
      || echo 'Warning: could not set the key partition list; codesign may ask for "Always Allow".' >&2
  fi
}

if [ "$mode" = import ]; then
  [ -f "$import_file" ] || { echo "No such file: $import_file" >&2; exit 1; }
  read_passphrase
  openssl pkcs12 -in "$import_file" -passin env:MONOPTAH_P12_PASS -nodes -nocerts -out "$work/key.pem" 2>/dev/null \
    || { echo 'Could not read the backup (wrong passphrase or not a PKCS#12 file).' >&2; exit 1; }
  openssl pkcs12 -in "$import_file" -passin env:MONOPTAH_P12_PASS -nokeys -clcerts -out "$work/cert.pem" 2>/dev/null
  openssl rsa -in "$work/key.pem" -traditional -out "$work/key-rsa.pem" 2>/dev/null
  import_pem "$work/key-rsa.pem" "$work/cert.pem"
else
  # Never overwrite a backup: it may be the only copy of the key.
  if [ -e "$backup" ]; then
    echo "The backup file already exists: $backup" >&2
    echo "Refusing to overwrite it. Restore with: $0 --import $backup (or pass another --backup PATH)." >&2
    exit 1
  fi
  read_passphrase
  cat > "$work/openssl.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $CN
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
subjectKeyIdentifier = hash
CNF
  openssl req -x509 -newkey rsa:2048 -nodes -days "$days" -sha256 \
    -config "$work/openssl.cnf" -keyout "$work/key.pem" -out "$work/cert.pem" 2>/dev/null
  openssl rsa -in "$work/key.pem" -traditional -out "$work/key-rsa.pem" 2>/dev/null
  # 3DES/SHA1 PBE keeps the backup readable by LibreSSL and OpenSSL 3 alike.
  openssl pkcs12 -export -certpbe PBE-SHA1-3DES -keypbe PBE-SHA1-3DES -macalg sha1 \
    -inkey "$work/key.pem" -in "$work/cert.pem" -name "$CN" \
    -passout env:MONOPTAH_P12_PASS -out "$work/identity.p12" 2>/dev/null
  # The backup is written first: if the import fails, the key is not lost.
  install -m 600 "$work/identity.p12" "$backup"
  echo "Backup written to: $backup (keep it private, never commit it, store the passphrase in your password manager)"
  import_pem "$work/key-rsa.pem" "$work/cert.pem"
fi
unset MONOPTAH_P12_PASS

found="$(existing_sha1)"
if [ -z "$found" ]; then
  echo "The identity was imported but \"$CN\" is not listed by security find-identity." >&2
  exit 1
fi
echo "SHA-1: $found"
echo "Use it with: export MONOPTAH_NATIVE_SIGNING_IDENTITY=$found"
