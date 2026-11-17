// The fixed catalog the headless desktop serves in `serve` mode (one fake project, one fake session). The pairing spec
// asserts it over the encrypted link. It is the contract between this harness and `web_link_headless serve`: when
// `serve` is wired (TODO(E-01:WS-D1 R-05, R-15)), it must answer `workspace.list` with exactly these names.
export const E2E_CATALOG = {
  projectName: "E2E Project",
  sessionTitle: "E2E session",
} as const;
