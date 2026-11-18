import { useCallback, useEffect, useState } from "react";
import { renderBackground } from "./render";
import { getBackground, listBackgrounds, putBackground, removeBackground, setBackgroundOpacity, subscribeBackgrounds, type BackgroundRecord } from "./store";
import { DEFAULT_OPACITY } from "./dither";

/** A stored background with an object URL for its image (revoked when replaced or on unmount). */
export interface LoadedBackground {
  record: BackgroundRecord;
  url: string;
}

/** The stored background of one project, kept current across components; null while loading or when none is set. */
export function useStoredBackground(projectId: string): LoadedBackground | null {
  const [loaded, setLoaded] = useState<LoadedBackground | null>(null);
  useEffect(() => {
    let live = true;
    let current: string | null = null;
    const show = (record: BackgroundRecord | null) => {
      if (current) URL.revokeObjectURL(current);
      current = record ? URL.createObjectURL(record.image) : null;
      setLoaded(record && current ? { record, url: current } : null);
    };
    const load = () => {
      getBackground(projectId).then(
        (next) => live && show(next),
        () => live && show(null),
      );
    };
    load();
    const unsubscribe = subscribeBackgrounds(load);
    return () => {
      live = false;
      unsubscribe();
      if (current) URL.revokeObjectURL(current);
    };
  }, [projectId]);
  return loaded;
}

/** Every stored background, by project name; kept current across components. */
export function useAllBackgrounds(): BackgroundRecord[] {
  const [records, setRecords] = useState<BackgroundRecord[]>([]);
  useEffect(() => {
    let live = true;
    const load = () => {
      listBackgrounds().then(
        (next) => live && setRecords(next),
        () => live && setRecords([]),
      );
    };
    load();
    const unsubscribe = subscribeBackgrounds(load);
    return () => {
      live = false;
      unsubscribe();
    };
  }, []);
  return records;
}

export interface BackgroundActions {
  busy: boolean;
  error: string | null;
  /** Dithers `file` and stores it for the project, keeping the current opacity. */
  pick: (file: File) => Promise<void>;
  setOpacity: (opacity: number) => Promise<void>;
  remove: () => Promise<void>;
}

export function useBackgroundActions(projectId: string, projectName: string, current: BackgroundRecord | null): BackgroundActions {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const opacity = current?.opacity ?? DEFAULT_OPACITY;

  const pick = useCallback(
    async (file: File) => {
      if (!file.type.startsWith("image/")) {
        setError("Choose an image file.");
        return;
      }
      setBusy(true);
      setError(null);
      try {
        const image = await renderBackground(file);
        await putBackground({ projectId, projectName, image, opacity, updatedAt: Date.now() });
      } catch {
        setError("Could not use that image.");
      } finally {
        setBusy(false);
      }
    },
    [projectId, projectName, opacity],
  );

  // A store that refuses (storage full or blocked) is said on screen, not dropped.
  const setOpacity = useCallback(
    async (value: number) => {
      try {
        await setBackgroundOpacity(projectId, value);
        setError(null);
      } catch {
        setError("Could not change the opacity.");
      }
    },
    [projectId],
  );
  const remove = useCallback(async () => {
    try {
      await removeBackground(projectId);
      setError(null);
    } catch {
      setError("Could not remove the background.");
    }
  }, [projectId]);
  return { busy, error, pick, setOpacity, remove };
}
