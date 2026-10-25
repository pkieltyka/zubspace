import assert from "node:assert/strict";
import test from "node:test";
import { fetchGameStatusResponse } from "../tools/game_status.mjs";

function payloads() {
  return {
    "/v1/status": {ok:true,service:"zubspace-server",total_players:2,clients:1,rtc_sessions:1},
    "/v1/zones": {total_players:2,zones:[{id:"chaos-zone",status:"open",default:true,total_players:2,free_for_all:true,maxed_spawn_loadout:true}]},
    "/v1/players": {total_players:2,players:[
      {id:1,name:"Ace",zone:"chaos-zone",bot:false},
      {id:768,name:"Mothman",zone:"chaos-zone",bot:true},
    ]},
    "/v1/zones/chaos-zone/status": {ok:true,zone:"chaos-zone",status:"open",total:2,snapshot_acked_clients:1,
      tick_timing:{work:{samples:3}},repel_queue_packets:0},
  };
}

test("QA joins explicit public scopes without restoring removed public aliases", async t => {
  const data=payloads(),calls=[];
  const controller=new AbortController();
  t.mock.method(globalThis,"fetch",async(url,options)=>{
    const parsed=new URL(url);calls.push({origin:parsed.origin,path:parsed.pathname,options});
    return Response.json(data[parsed.pathname]);
  });
  const response=await fetchGameStatusResponse("http://127.0.0.1:9001/v1/status",{signal:controller.signal});
  const view=await response.json();
  assert.equal(view.joinedPlayers,1);assert.equal(view.botPlayers,1);
  assert.equal(view.snapshot_acked_clients,1);assert.equal(view.zone,"chaos-zone");
  assert.deepEqual(view.tick_timing,data["/v1/zones/chaos-zone/status"].tick_timing);
  assert.equal(view.total,2);assert.equal(view.zoneTotal,2);
  assert.equal(view.free_for_all,true);assert.equal(view.maxed_spawn_loadout,true);
  assert.equal(view.players,undefined);assert.equal(view.bots,undefined);
  assert.equal(view.rtc_snapshot_acked_clients,undefined);
  assert.equal(view.qaStatusSource.simulation,"/v1/zones/chaos-zone/status");
  assert.deepEqual(calls.map(call=>call.path),Object.keys(data));
  assert(calls.every(call=>call.origin==="http://127.0.0.1:9001" && call.options.signal===controller.signal && call.options.cache==="no-store"));
});

test("independent HTTP samples tolerate an intervening join without inventing atomicity", async t => {
  const data=payloads();data["/v1/status"].total_players=1;
  data["/v1/zones/chaos-zone/status"].total=3;
  t.mock.method(globalThis,"fetch",async url=>Response.json(data[new URL(url).pathname]));
  const view=await(await fetchGameStatusResponse("http://127.0.0.1:9001/v1/status")).json();
  assert.equal(view.total,1);assert.equal(view.zoneTotal,3);assert.equal(view.joinedPlayers,1);
});

test("old schemas, incomplete zone status and malformed player classification fail closed", async t => {
  let data;
  t.mock.method(globalThis,"fetch",async url=>Response.json(data[new URL(url).pathname]));
  for(const corrupt of [
    d=>{d["/v1/status"]={ok:true,service:"zubspace-server",zone:"chaos-zone",players:1,bots:1};},
    d=>{d["/v1/zones"].zones[0].default=false;},
    d=>{d["/v1/players"].total_players=3;},
    d=>{d["/v1/players"].players[1].bot="true";},
    d=>{d["/v1/players"].players[1].id=1;},
    d=>{d["/v1/players"].players[1].zone=["chaos-zone"];},
    d=>{d["/v1/zones"].zones[0].id=["chaos-zone"];},
    d=>{delete d["/v1/zones/chaos-zone/status"].snapshot_acked_clients;},
    d=>{d["/v1/zones/chaos-zone/status"].zone="extreme";},
  ]) {
    data=payloads();corrupt(data);
    await assert.rejects(fetchGameStatusResponse("http://127.0.0.1:9001/v1/status"),/QA status/);
  }
});

test("native process failures are retained and ambiguous endpoints never fetch", async t => {
  const data=payloads();let calls=0;
  t.mock.method(globalThis,"fetch",async url=>{
    calls++;return new URL(url).pathname==="/v1/status" ? new Response("unavailable",{status:503}) : Response.json(data[new URL(url).pathname]);
  });
  const result=await fetchGameStatusResponse("http://127.0.0.1:9001/v1/status");
  assert.equal(result.status,503);assert.equal(await result.text(),"unavailable");
  const before=calls;
  for(const url of ["http://localhost/v1/status?zone=extreme","http://user:pw@localhost/v1/status","http://localhost/status","http://localhost/v1/status#x"]) {
    await assert.rejects(fetchGameStatusResponse(url),/exact process status/);
  }
  assert.equal(calls,before);
});
