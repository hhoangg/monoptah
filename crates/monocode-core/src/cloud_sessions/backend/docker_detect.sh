set -eu
# Runs in the OUTER sandbox, in the fresh checkout. The app prepends WS and CONFIG (the
# configured devcontainer path, or empty for the default one). Prints
# MONOPTAH_DEVCONTAINER=config when the repository has a devcontainer config, `none` when it
# has none (the session then runs directly in the sandbox) and `missing` when a configured
# non-default path does not exist. The default lookup also accepts `.devcontainer.json` at
# the repository root, as the devcontainer spec does.
cd "$WS"
if [ -n "$CONFIG" ]; then
  if [ -f "$CONFIG" ]; then
    echo 'MONOPTAH_DEVCONTAINER=config'
  else
    echo 'MONOPTAH_DEVCONTAINER=missing'
  fi
elif [ -f .devcontainer/devcontainer.json ] || [ -f .devcontainer.json ]; then
  echo 'MONOPTAH_DEVCONTAINER=config'
else
  echo 'MONOPTAH_DEVCONTAINER=none'
fi
