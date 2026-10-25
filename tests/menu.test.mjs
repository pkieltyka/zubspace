import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";
import test from "node:test";
const scope = vm.createContext({});
vm.runInContext(fs.readFileSync("js/zubspace_menu.js", "utf8"), scope);
const zone = (query) => scope.zubspaceMenuZoneHint(new URLSearchParams(query));
test("public menu defaults to Chaos and preserves explicit hidden zone URLs", () => {
  assert.equal(zone(""), "chaos-zone");
  assert.equal(zone("zone=extreme-games"), "extreme");
  assert.equal(zone("zone=Extreme%20Games&arena=chaos-zone"), "extreme");
  assert.equal(zone("arena=extreme"), "extreme");
  assert.equal(zone("zone=test-zone"), "test-zone");
});
test("menu shows the cross-zone total and rejects unavailable counts", () => {
  const catalog = {total_players:6,zones:[{id:"chaos-zone",total_players:4},{id:"extreme",total_players:2}]};
  assert.deepEqual(JSON.parse(JSON.stringify(scope.zubspaceMenuPopulation(catalog))), {players:6});
  for(const invalid of [null,{}, {...catalog,total_players:-1},{...catalog,total_players:"6"},
    {...catalog,total_players:0.5},{...catalog,total_players:Infinity},{ok:true,zone:"chaos-zone",players:2,bots:4}]) {
    assert.equal(scope.zubspaceMenuPopulation(invalid),null);
  }
  assert.equal(scope.zubspaceMenuPopulation({...catalog,total_players:0}).players,0);
});
test("menu hides unlisted zones while retaining an explicit deep link", () => {
  const zones=[{id:"chaos-zone",listed:true},{id:"extreme",listed:false},{id:"custom",listed:false}];
  assert.deepEqual(JSON.parse(JSON.stringify(scope.zubspaceMenuZoneChoices(zones,"chaos-zone"))),[zones[0]]);
  assert.deepEqual(JSON.parse(JSON.stringify(scope.zubspaceMenuZoneChoices(zones,"extreme"))),zones.slice(0,2));
});

test("desktop-only join detects phones and iPads without rejecting touch laptops or small windows", () => {
  const mobile = scope.zubspaceMenuIsMobileDevice;
  assert.equal(mobile({ userAgent: "iPhone; CPU iPhone OS 18_0 like Mac OS X" }), true);
  assert.equal(mobile({ userAgent: "iPad; CPU OS 18_0 like Mac OS X" }), true);
  assert.equal(mobile({ userAgent: "Linux; Android 15; Tablet" }), true);
  assert.equal(mobile({ userAgentData: { mobile: true } }), true);
  assert.equal(mobile({ userAgent: "Macintosh; Intel Mac OS X", platform: "MacIntel", maxTouchPoints: 5 }), true);
  assert.equal(mobile({ platform: "MacIntel", maxTouchPoints: 0 }), false);
  assert.equal(mobile({ platform: "Win32", maxTouchPoints: 10, userAgentData: { mobile: false } }), false);
  assert.equal(mobile({ platform: "Linux x86_64" }), false);
});
