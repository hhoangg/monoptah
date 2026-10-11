import { useSyncExternalStore } from "react";
import {
  loadMailEnabled,
  saveMailEnabled,
  subscribeMailEnabled,
} from "../model/settings";
import { Group, Row, Toggle } from "./primitives";

// Placeholder: the provider and source UI replaces this page.
export function MailSettings() {
  const enabled = useSyncExternalStore(
    subscribeMailEnabled,
    loadMailEnabled,
    () => false,
  );
  return (
    <Group title="Mail">
      <Row
        id="mail-enabled"
        label="Enable Mail"
        description="Show Mail in the sidebar."
      >
        <Toggle label="Enable Mail" on={enabled} onChange={saveMailEnabled} />
      </Row>
    </Group>
  );
}
