import { useCapabilities } from "@/lib/api/queries";

/** Loud banner whenever a development hardware fixture is simulated. */
export function SimulationBanner() {
  const { data } = useCapabilities();
  if (!data?.simulatedFixture) return null;
  return (
    <div role="status" className="bg-warning px-4 py-1.5 text-center text-xs font-medium text-black">
      Simulating hardware fixture “{data.simulatedFixture}”. Capability results do not describe this computer.
    </div>
  );
}
