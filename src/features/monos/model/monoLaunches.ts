export type MonoLaunch = { monoId: string; parented: boolean };

/** Who started a session through the app CLI, kept for the life of the app. */
export type MonoLaunchRegistry = {
  record(sessionId: string, monoId: string, parented: boolean): void;
  get(sessionId: string): MonoLaunch | undefined;
};

export function createMonoLaunchRegistry(): MonoLaunchRegistry {
  const launches = new Map<string, MonoLaunch>();
  return {
    record(sessionId, monoId, parented) {
      launches.set(sessionId, { monoId, parented });
    },
    get: (sessionId) => launches.get(sessionId),
  };
}
