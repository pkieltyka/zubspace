import { parentPort, workerData } from "node:worker_threads";
import { c2sRecoveryLogSummary } from "./combat_replay.mjs";

const { logText, playerId, participantIds } = workerData;
parentPort.postMessage(c2sRecoveryLogSummary({ logs: [logText] }, 0, playerId, participantIds));
parentPort.close();
