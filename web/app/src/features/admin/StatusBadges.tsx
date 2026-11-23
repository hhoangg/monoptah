import { Badge } from "@/ui/badge";
import type { AdminUser } from "./adminApi";

export function StatusBadges({ user }: { user: AdminUser }) {
  return (
    <>
      {user.role === "admin" ? <Badge variant="outline">Admin</Badge> : null}
      {user.status === "blocked" ? <Badge variant="destructive">Blocked</Badge> : <Badge variant="outline">Active</Badge>}
    </>
  );
}
