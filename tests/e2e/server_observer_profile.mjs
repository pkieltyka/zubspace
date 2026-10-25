export function serverObserverProfileForNetworkProfile(networkProfile) {
  switch (networkProfile) {
    case "":
      return "clean-lan";
    case "mild":
      return "supported-mild-wan";
    case "slow":
      return "supported-slow-wan";
    default:
      throw new Error(
        `unsupported browser impairment profile for server observer policy: ${JSON.stringify(networkProfile)}`,
      );
  }
}

export function assertServerObserverProfile(status, expectedProfile) {
  const actualProfile = status?.observer_network_profile;
  if (actualProfile !== expectedProfile) {
    throw new Error(
      `server observer profile mismatch: expected ${expectedProfile}, got ${JSON.stringify(actualProfile)}`,
    );
  }
  return {
    authority: "trusted-server-config",
    expectedProfile,
    actualProfile,
  };
}
