// Behavioral reference controls execute the pinned official SDK helpers.
import {createSettings, createMentions, createElicitInput} from "./upstream/typescript/src/server/index.ts";
import {z} from "zod";
function server() {
  const tools = new Map();
  const capabilities = [];
  return {tools, capabilities, server:{transport:undefined, registerCapabilities:value=>capabilities.push(value)},
    registerTool(name, config, callback) {
      if (tools.has(name)) throw new Error("duplicate tool");
      tools.set(name,{config,callback});
      return {remove:()=>tools.delete(name)};
    }};
}
async function call(s,name,args) {
  const {config,callback}=s.tools.get(name);
  return callback(config.inputSchema.parse(args),{});
}
const failed=[];
const cases=[];
async function check(name, callback) {
  try {await callback(); cases.push(name);}
  catch(error) {failed.push({name,error:String(error)});}
}
function equal(actual,expected) {
  if(JSON.stringify(actual)!==JSON.stringify(expected)) throw new Error(JSON.stringify({actual,expected}));
}
async function rejects(callback) {
  let rejected=false;
  try{await callback();}catch{rejected=true;}
  equal(rejected,true);
}
function settings(read,update) {
  return {fields:{units:{schema:z.enum(["mm","in"]),title:"Units"},showGrid:{schema:z.boolean().optional(),title:"Grid"}},read,update};
}
await check("settings_contract",async()=>{
  const s=server();
  createSettings(s).register(settings(()=>({units:"mm",showGrid:true}),set=>({units:"mm",showGrid:true,...set})));
  equal([...s.tools.keys()],["settings.read","settings.update"]);
  equal(s.tools.get("settings.read").config.annotations.readOnlyHint,true);
  equal(s.capabilities,[{extensions:{"openai/settings":{readTool:"settings.read",updateTool:"settings.update"}},experimental:{"openai/settings":{readTool:"settings.read",updateTool:"settings.update"}}}]);
  const result=await call(s,"settings.read",{});
  equal(result.content,[]);
  equal(result.structuredContent.values,{units:"mm",showGrid:true});
});
await check("invalid_updates_never_call_handler",async()=>{
  const s=server();let count=0;
  createSettings(s).register(settings(()=>({units:"mm",showGrid:true}),set=>{count++;return {units:"mm",showGrid:true,...set};}));
  for(const args of [{set:{}},{set:{units:"cm"}},{set:{unknown:true}},{set:{showGrid:"yes"}}]) await rejects(()=>call(s,"settings.update",args));
  equal(count,0);
});
await check("all_read_values_required",async()=>{
  const s=server();
  createSettings(s).register(settings(()=>({units:"mm"}),()=>({units:"mm",showGrid:true})));
  await rejects(()=>call(s,"settings.read",{}));
});
await check("unknown_read_values_rejected",async()=>{
  const s=server();
  createSettings(s).register(settings(()=>({units:"mm",showGrid:true,extra:true}),()=>({units:"mm",showGrid:true})));
  await rejects(()=>call(s,"settings.read",{}));
});
await check("settings_defaults_rejected",async()=>{
  const s=server();
  await rejects(()=>createSettings(s).register({fields:{enabled:{schema:z.boolean().default(true),title:"Enabled"}},read:()=>({enabled:true}),update:()=>({enabled:true})}));
  equal(s.tools.size,0);
});
await check("settings_duplicate_registration",async()=>{
  const s=server();const facade=createSettings(s);
  facade.register(settings(()=>({units:"mm",showGrid:true}),()=>({units:"mm",showGrid:true})));
  await rejects(()=>facade.register(settings(()=>({units:"mm",showGrid:true}),()=>({units:"mm",showGrid:true}))));
  equal([...s.tools.keys()],["settings.read","settings.update"]);
});
await check("settings_registration_rolls_back",async()=>{
  const s=server();const original=s.registerTool;
  s.registerTool=(name,...rest)=>{if(name==="settings.update")throw new Error("registration failure");return original(name,...rest);};
  await rejects(()=>createSettings(s).register(settings(()=>({units:"mm",showGrid:true}),()=>({units:"mm",showGrid:true}))));
  equal(s.tools.size,0);
});
await check("mentions_replace_handler_once",async()=>{
  const s=server();const facade=createMentions(s);
  facade.setHandler(()=>({items:[]}));
  facade.setHandler(({query})=>({items:[{type:"resource_link",uri:"cad://"+query,name:query}]}));
  equal([...s.tools.keys()],["search_mentions"]);
  equal(s.tools.get("search_mentions").config._meta,{"openai/extensions":{"mentions/search":{}},ui:{visibility:["app"]}});
  equal((await call(s,"search_mentions",{query:"bolt"})).structuredContent,{items:[{type:"resource_link",uri:"cad://bolt",name:"bolt"}]});
});
await check("elicitation_requires_capability",async()=>{
  let calls=0;
  const elicit=createElicitInput({server:{getClientCapabilities:()=>({}),request:()=>{calls++;return Promise.resolve({action:"cancel"});}}});
  await rejects(()=>elicit({mode:"form",message:"Name",requestedSchema:{type:"object",properties:{name:{type:"string"}},required:["name"]}}));
  equal(calls,0);
});
await check("elicitation_validates_answer_without_defaults",async()=>{
  let answer="a";const requests=[];
  const elicit=createElicitInput({server:{getClientCapabilities:()=>({extensions:{"openai/elicitation":{form:{}}}}),
    request:(request)=>{requests.push(request);return Promise.resolve({action:"accept",content:{name:answer}});}}});
  const params={mode:"form",message:"Name",requestedSchema:{type:"object",properties:{name:{type:"string",pattern:"^a+$"}},required:["name"]}};
  equal(await elicit(params),{action:"accept",content:{name:"a"}});
  answer="b";await rejects(()=>elicit(params));
  equal(requests.map(r=>r.method),["openai/elicitation/create","openai/elicitation/create"]);
});
for(const [name,schema,good,bad] of [
  ["utf16_length",z.string().min(2).max(4),"😀",["a","abcde"]],
  ["ecmascript_pattern",z.string().regex(/(?<=a)b/),"ab",["bb","a"]],
  ["email",z.email(),"a@example.com",["not-an-email"]],
  ["numeric_bounds",z.number().min(1).max(3),2,[0,4]],
  ["exclusive_bounds",z.number().gt(0).lt(5),2,[0,5]],
  ["multiple_of",z.number().multipleOf(0.5),2,[1.25]],
  ["decimal_multiple",z.number().multipleOf(0.1),0.3,[0.31]],
  ["safe_integer",z.int(),1,[1.5,9007199254740992]],
  ["string_literal",z.literal("fixed"),"fixed",["other",true]],
  ["boolean_literal",z.literal(true),true,[false,"true"]],
  ["number_literal",z.literal(2),2,[1,3,"2"]]
]) {
  await check("settings_"+name,async()=>{
    const s=server();let calls=0;
    createSettings(s).register({fields:{value:{schema,title:"Value"}},
      read:()=>({value:good}),update:set=>{calls++;return {value:good,...set};}});
    for(const value of bad)await rejects(()=>call(s,"settings.update",{set:{value}}));
    equal(calls,0);
    equal((await call(s,"settings.update",{set:{value:good}})).structuredContent.values,{value:good});
    equal(calls,1);
  });
}
await check("elicitation_exact_extension_capability",async()=>{
  let calls=0;
  const params={mode:"form",message:"Name",requestedSchema:{type:"object",properties:{name:{type:"string"}},required:["name"]}};
  for(const capabilities of [{},{elicitation:{}},{experimental:{"openai/elicitation":{form:{}}}},
    ...[null,false,true,[],0,""].map(form=>({extensions:{"openai/elicitation":{form}}}))]) {
    const elicit=createElicitInput({server:{getClientCapabilities:()=>capabilities,
      request:()=>{calls++;return Promise.resolve({action:"cancel"});}}});
    await rejects(()=>elicit(params));
  }
  equal(calls,0);
});
console.log(JSON.stringify({cases:cases.length,passed:cases,failed}));
process.exitCode=failed.length?1:0;
