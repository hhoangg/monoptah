import type { LinkConnection } from "@/link";

/** The part of `LinkConnection` the data layer uses (tests pass a fake). */
export type LinkClient = Pick<LinkConnection, "desktopId" | "store" | "request" | "registerWatch" | "onResynced">;

/* throwaway: native path filter test */
