import type { ComponentProps } from "react";
import { cn } from "@/lib/utils";

// Card (design-system.md section 8): bg surface, radius 22, padding 14, no shadow and no border.
export function Card({ className, ...props }: ComponentProps<"div">) {
  return <div data-slot="card" className={cn("rounded-card bg-card p-3.5 text-card-foreground", className)} {...props} />;
}

export function CardTitle({ className, ...props }: ComponentProps<"h2">) {
  return <h2 data-slot="card-title" className={cn("text-body font-semibold", className)} {...props} />;
}

export function CardDescription({ className, ...props }: ComponentProps<"p">) {
  return <p data-slot="card-description" className={cn("text-meta text-muted-text", className)} {...props} />;
}
