import { expect, it, vi } from "vitest";
import { createSessionThen } from "./newSessionInFolder";

it("places a synchronously created session once", () => {
  const place = vi.fn();
  createSessionThen((onCreated) => {
    onCreated("s1");
    return "s1";
  }, place);
  expect(place).toHaveBeenCalledTimes(1);
  expect(place).toHaveBeenCalledWith("s1");
});

it("places a creator that only returns the id", () => {
  const place = vi.fn();
  createSessionThen(() => "s2", place);
  expect(place).toHaveBeenCalledWith("s2");
});

it("places a deferred session only once the continuation fires", () => {
  const place = vi.fn();
  let fire: (id: string) => void = () => {};
  createSessionThen((onCreated) => {
    fire = onCreated;
  }, place);
  expect(place).not.toHaveBeenCalled();
  fire("s3");
  expect(place).toHaveBeenCalledWith("s3");
  expect(place).toHaveBeenCalledTimes(1);
});

it("never places when the deferred prompt is cancelled", () => {
  const place = vi.fn();
  createSessionThen(() => undefined, place);
  expect(place).not.toHaveBeenCalled();
});
