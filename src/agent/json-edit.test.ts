import {expect,test} from "bun:test";
import {editJson, type JsonChange} from "./json-edit";

test("JSON pointers address escaped keys and preserve unrelated values", () => {
  const source=JSON.stringify({"a/b":{"~key":1},keep:{value:2}});
  const next=JSON.parse(editJson(source,[{op:"replace",pointer:"/a~1b/~0key",expected:1,value:3},{op:"add",pointer:"/keep/nullable",value:true},{op:"remove",pointer:"/keep/value",expected:2}]));
  expect(next).toEqual({"a/b":{"~key":3},keep:{nullable:true}});
});

test("JSON edits cannot cross inherited properties or change ambiguous array positions", () => {
  const changes: JsonChange[] = [{op:"add",pointer:"/__proto__/polluted",value:true},{op:"replace",pointer:"/rows/01",expected:"a",value:"b"},{op:"add",pointer:"/rows/0",value:"b"}];
  for(const change of changes){
    expect(()=>editJson('{"rows":["a"]}',[change])).toThrow();
  }
  expect(Object.hasOwn(Object.prototype,"polluted")).toBe(false);
});
