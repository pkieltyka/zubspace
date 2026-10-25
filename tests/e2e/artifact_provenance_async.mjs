import { isMainThread, parentPort, Worker, workerData } from "node:worker_threads";
import { collectArtifactProvenance } from "./artifact_provenance.mjs";

const task = "zubspace-artifact-provenance";

if (!isMainThread && workerData?.task === task) {
  parentPort.postMessage(collectArtifactProvenance(workerData.options));
  parentPort.close();
}

// A live browser runner also drains the server's log pipes. Synchronous tree
// hashing can block that drain long enough to stall the server and lose ICE.
// Keep identical identity checks in a short-lived worker, outside that loop.
export function collectArtifactProvenanceAsync(options = {}) {
  return new Promise((resolve, reject) => {
    const worker = new Worker(new URL(import.meta.url), {
      workerData: { task, options }, execArgv: [],
    });
    let result;
    worker.once("message", (message) => { result = message; });
    worker.once("error", reject);
    worker.once("exit", (code) => {
      if (code === 0 && result) resolve(result);
      else reject(new Error(`artifact provenance worker exited without a result (code ${code})`));
    });
  });
}
