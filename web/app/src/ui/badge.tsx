import { cva, type VariantProps } from "class-variance-authority";
import type { ComponentProps } from "react";
import { cn } from "./cn";

const badgeVariants = cva("inline-flex min-h-5 min-w-5 items-center justify-center rounded-[10px] px-1.5 text-caption font-semibold tabular", {
  variants: {
    variant: {
      default: "bg-fill text-content",
      outline: "border border-stroke text-muted-text",
      brand: "bg-brand text-inverse",
      destructive: "bg-destructive text-inverse",
    },
  },
  defaultVariants: { variant: "default" },
});

export function Badge({ className, variant, ...props }: ComponentProps<"span"> & VariantProps<typeof badgeVariants>) {
  return <span data-slot="badge" className={cn(badgeVariants({ variant }), className)} {...props} />;
}
