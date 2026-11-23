/**
 * A block's status is the provider's own string, and the engine fails safe on one it
 * does not know.
 *
 * `docs/api.md` publishes the asymmetry for both generated model sets: a block's
 * status is provider data, so the models advertise the open string the wire layer
 * reads rather than the closed three the engine enumerates, while a status-policy
 * entry's own `status` stays closed because it is the caller's configuration. The
 * runtime half — a provider's `workingElsewhere` reading as `busy` instead of as free
 * time — is asserted below. The typing half is the document itself: it is annotated as
 * {@link Request}, so a generated type that closed `Block.status` fails
 * `npm run check` here rather than in a consumer's build, the way
 * `null_defaults.test.ts` pins the null arms.
 */

import assert from "node:assert/strict";
import test from "node:test";

import { Engine, type Request } from "opportune";

/** A provider's own status, outside the three the engine enumerates. */
const PROVIDER_STATUS = "workingElsewhere";

/**
 * A request whose only block covers `ana`'s whole search window, carrying `status`.
 *
 * `status` is a plain `string` rather than the engine's three, which is what makes the
 * annotation below the assertion: a closed generated type rejects this document.
 */
function request(status: string): Request {
  return {
    now: "2027-03-09T15:00Z",
    goal: {
      parts: [{ duration: "30m", requirement: { all: [{ person: "ana" }] } }],
      search_window: ["2027-03-09T15:00Z", "2027-03-09T16:00Z"],
      zone: "UTC",
    },
    participants: [
      {
        id: "ana",
        zone: "UTC",
        calendar: [
          { span: ["2027-03-09T15:00Z", "2027-03-09T16:00Z"], status, tags: [] },
        ],
      },
    ],
  };
}

test("a provider status outside the three is typed, reads, and fails closed as busy", () => {
  const engine = new Engine();

  assert.ok(
    engine.findAvailability(request("free")).offers.length > 0,
    "a free block leaves the hour open",
  );
  assert.equal(
    engine.findAvailability(request(PROVIDER_STATUS)).offers.length,
    0,
    "a status the engine does not know fails safe as busy, never as free time",
  );
});
