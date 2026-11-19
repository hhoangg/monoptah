// The fixed catalog the headless desktop serves in `serve` mode (one fake project, one fake session). The specs
// assert it over the encrypted link. It is the contract between this harness and `web_link_headless serve`
// (crates/monocode-core/src/bin/web_link_headless.rs): it answers `workspace.list` with exactly these values.
export const E2E_CATALOG = {
  projectId: "e2e-project",
  projectName: "E2E Project",
  sessionId: "e2e-session",
  sessionTitle: "E2E session",
} as const;
