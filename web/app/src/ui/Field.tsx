import { useId, type ComponentProps, type ReactNode } from "react";
import { cn } from "./cn";
import { Input } from "./input";

export interface FieldProps extends Omit<ComponentProps<"input">, "id"> {
  label: string;
  /** Helper text under the input. */
  hint?: ReactNode;
  /** Validation message; replaces nothing, shown below the hint with `role="alert"`. */
  error?: string | null;
  id?: string;
  wrapperClassName?: string;
}

/** Label, input (17px), hint and error wired together with `aria-describedby` and `aria-invalid`. */
export function Field({ label, hint, error, id, wrapperClassName, className, ...input }: FieldProps) {
  const generated = useId();
  const inputId = id ?? generated;
  const hintId = `${inputId}-hint`;
  const errorId = `${inputId}-error`;
  const describedBy = [hint ? hintId : null, error ? errorId : null].filter(Boolean).join(" ") || undefined;
  return (
    <div className={cn("flex flex-col gap-1.5", wrapperClassName)}>
      <label htmlFor={inputId} className="text-label font-medium text-content">
        {label}
      </label>
      <Input id={inputId} aria-invalid={error ? true : undefined} aria-describedby={describedBy} className={className} {...input} />
      {hint ? (
        <p id={hintId} className="text-caption text-muted-text">
          {hint}
        </p>
      ) : null}
      {error ? (
        <p id={errorId} role="alert" className="text-meta text-deletions">
          {error}
        </p>
      ) : null}
    </div>
  );
}
