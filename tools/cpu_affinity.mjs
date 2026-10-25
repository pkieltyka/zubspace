// Optional Linux test-process isolation. No shell interpretation, gameplay
// changes, or implicit CPU selection; the caller records the requested mask.
export function commandWithCpuSet(command, args, cpuSet = null) {
  if (cpuSet == null) return {command, args};
  if (typeof cpuSet !== "string" || cpuSet.length > 4096 ||
      !/^\d+(?:-\d+)?(?:,\d+(?:-\d+)?)*$/.test(cpuSet) ||
      cpuSet.split(",").some(part => {
        const [start, end = start] = part.split("-").map(Number);
        return !Number.isSafeInteger(start) || !Number.isSafeInteger(end) || start > end;
      })) throw new Error("invalid explicit test CPU set");
  return {command: "taskset", args: ["--cpu-list", cpuSet, command, ...args]};
}
