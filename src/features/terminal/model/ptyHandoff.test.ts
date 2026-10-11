import { beforeEach, expect, it } from "vitest";
import {
  claimPtyArrival,
  consumePtyHandoff,
  markPtyHandoff,
  settlePtyHandoffs,
  takeArrivedExit,
} from "./ptyHandoff";

beforeEach(settlePtyHandoffs);

it("consumes a departure mark once", () => {
  markPtyHandoff(["a", "b"]);
  expect(consumePtyHandoff("a")).toBe(true);
  expect(consumePtyHandoff("a")).toBe(false);
  expect(consumePtyHandoff("b")).toBe(true);
});

it("ignores an id that was never marked", () => {
  expect(consumePtyHandoff("none")).toBe(false);
});

it("hands the arrival to the next mount, once", () => {
  markPtyHandoff(["a"]);
  consumePtyHandoff("a");
  expect(claimPtyArrival("a")).toBe(true);
  expect(claimPtyArrival("a")).toBe(false);
});

it("gives no arrival for an id that was marked but never unmounted", () => {
  markPtyHandoff(["a"]);
  expect(claimPtyArrival("a")).toBe(false);
});

it("forgets a mark that nobody consumed once the commit has settled", () => {
  markPtyHandoff(["a"]);
  settlePtyHandoffs();
  expect(consumePtyHandoff("a")).toBe(false);
});

it("forgets an arrival that nobody claimed once the commit has settled", () => {
  markPtyHandoff(["a"]);
  consumePtyHandoff("a");
  settlePtyHandoffs();
  expect(claimPtyArrival("a")).toBe(false);
});

it("carries the exit a departing view already reported, once", () => {
  markPtyHandoff(["a"]);
  consumePtyHandoff("a", { code: 2 });
  expect(takeArrivedExit("a")).toEqual({ code: 2 });
  expect(takeArrivedExit("a")).toBeUndefined();
});

it("carries no exit for a process that was still running", () => {
  markPtyHandoff(["a"]);
  consumePtyHandoff("a");
  expect(takeArrivedExit("a")).toBeUndefined();
});

it("forgets a carried exit that nobody took once the commit has settled", () => {
  markPtyHandoff(["a"]);
  consumePtyHandoff("a", { code: null, error: "boom" });
  settlePtyHandoffs();
  expect(takeArrivedExit("a")).toBeUndefined();
});
