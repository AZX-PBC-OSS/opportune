/**
 * A personal booking link: one host, one thirty-minute meeting.
 *
 * Ten lines from a request to the earliest bookable start, against the package's
 * own types. Type-checked with the package (and kept out of `dist/`); the Node
 * suite executes the same request against the built module.
 */

import {
  Engine,
  businessHours,
  buffers,
  minNotice,
  offerStart,
  person,
  singleGoal,
  type Request,
} from "../src/index.js";

const request: Request = {
  now: "2027-06-14T07:00Z",
  goal: singleGoal("30m", { all: [person("rich")] }, {
    zone: "Europe/London",
    searchWindow: ["2027-06-13T23:00Z", "2027-06-18T23:00Z"],
  }),
  participants: [{ id: "rich", zone: "Europe/London", calendar: [] }],
  rules: [businessHours(), buffers(), minNotice()],
};

using engine = new Engine();
const [earliest] = engine.findAvailability(request).offers;
console.log(earliest === undefined ? "no offers" : offerStart(earliest));
