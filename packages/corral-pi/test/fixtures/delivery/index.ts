// Fixture delivery module: records every submit_input it is handed, in memory and as a
// session custom entry, so tests can read what reached the host.
import type { DeliveryInput, DeliveryModule } from "../../../src/delivery/types.ts";

const records: Omit<DeliveryInput, "text">[] = [];

const delivery: DeliveryModule = {
  accept(input, host) {
    const { text: _text, ...entry } = input;
    records.push(entry);
    host.appendEntry("corral_fixture_delivery", { ...entry });
  },
  state() {
    return { fixture: true, records: records.map((entry) => ({ ...entry })) };
  },
};

export default delivery;
