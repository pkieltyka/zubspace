import { Worker } from "node:worker_threads";

// Clone the already frozen scoring window once. Keep the driver's event loop
// draining RTC server stdout while the unchanged reductions parse that window.
export function c2sRecoveryLogSummaryAsync(options) {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL("./c2s_recovery_analysis_worker.mjs", import.meta.url), {
      workerData: options, execArgv: [],
    });
    let result;
    const timer = setTimeout(() => {
      reject(new Error("C2S recovery analysis exceeded its30s budget"));
      void worker.terminate();
    }, 30_000);
    worker.once("message", message => { result = message; });
    worker.once("error", reject);
    worker.once("exit", code => {
      clearTimeout(timer);
      if (code === 0 && result) resolve(result);
      else reject(new Error(`C2S recovery analysis exited without a result (code ${code})`));
    });
  });
}
