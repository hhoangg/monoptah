// Shared springs of design-system.md 6.1 (`motion` takes the same three numbers as Reanimated).
export const springs = {
  press: { type: "spring", stiffness: 100, damping: 15, mass: 0.8 },
  pop: { type: "spring", stiffness: 190, damping: 14, mass: 0.9 },
  pager: { type: "spring", stiffness: 1000, damping: 500, mass: 3 },
  mini: { type: "spring", stiffness: 210, damping: 17, mass: 0.8 },
  deck: { type: "spring", stiffness: 180, damping: 18, mass: 0.9 },
} as const;
