// The invite tree, derived from `users.invitedBy` (security.md 4). The API has no admin listing of invite
// codes, so the tree is built from who invited whom.
import type { AdminUser } from "./adminApi";

export interface TreeNode {
  user: AdminUser;
  children: TreeNode[];
}

/** Roots are users nobody in the list invited (the bootstrap admin, or an inviter that is gone). */
export function buildInviteTree(users: readonly AdminUser[]): TreeNode[] {
  const byId = new Map(users.map((user) => [user.id, user]));
  const children = new Map<string, AdminUser[]>();
  const roots: AdminUser[] = [];
  for (const user of users) {
    if (user.invitedBy !== null && user.invitedBy !== user.id && byId.has(user.invitedBy)) {
      const list = children.get(user.invitedBy) ?? [];
      list.push(user);
      children.set(user.invitedBy, list);
    } else {
      roots.push(user);
    }
  }
  const seen = new Set<string>();
  const build = (user: AdminUser): TreeNode => {
    seen.add(user.id);
    return { user, children: (children.get(user.id) ?? []).filter((child) => !seen.has(child.id)).map(build) };
  };
  return roots.map(build);
}

/** Everyone below `id` in the invite tree, at any depth (what a cascade block would also block). */
export function descendantsOf(users: readonly AdminUser[], id: string): AdminUser[] {
  const out: AdminUser[] = [];
  const seen = new Set<string>([id]);
  const queue = [id];
  while (queue.length > 0) {
    const current = queue.shift() as string;
    for (const user of users) {
      if (user.invitedBy === current && !seen.has(user.id)) {
        seen.add(user.id);
        out.push(user);
        queue.push(user.id);
      }
    }
  }
  return out;
}

export function handleOf(users: readonly AdminUser[], id: string | null): string | null {
  if (id === null) return null;
  const found = users.find((user) => user.id === id);
  return found ? `@${found.handle}` : null;
}
