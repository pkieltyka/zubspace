// Explicit test launch configuration; never infer hardware acceleration from a flag.
export function chromeGraphicsArguments(backend = 'swiftshader') {
  if (backend === 'native') return [];
  if (backend === 'swiftshader') return ['--enable-unsafe-swiftshader', '--use-gl=angle', '--use-angle=swiftshader'];
  if (backend === 'vulkan') return ['--use-gl=angle', '--use-angle=vulkan'];
  throw new Error(`unsupported Chrome graphics backend: ${backend}`);
}
