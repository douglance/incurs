import {EmptyResultSchema, ReadResourceResultSchema} from "@modelcontextprotocol/sdk/types.js";
import {createResources} from "./upstream/typescript/src/app/resources.ts";
const empty = EmptyResultSchema.parse({_meta:{trace:"file-open"}});
let sent;
const resources = createResources({readServerResource:async params => { sent = params; return ReadResourceResultSchema.parse({contents:[{uri:"file://a",text:"hello",_meta:{"openai/resource":{etag:"e1"}}}],_meta:{trace:"resource-read"}}); }});
const result = await resources.read({uri:"file://a",representation:"text",_meta:{trace:"outbound","openai/resource":{representation:"blob"}}});
if (empty._meta.trace !== "file-open" || sent._meta.trace !== "outbound" || result._meta.trace !== "resource-read") throw new Error("metadata preservation failed");
let invalidRejected=false; try {ReadResourceResultSchema.parse({contents:[{uri:"file://a"}]});} catch {invalidRejected=true;}
if(!invalidRejected) throw new Error("content shape control failed");
for (const content of [{uri:"file://a",text:"hello",blob:true},{uri:"file://a",blob:"AA",text:42}]) {
  ReadResourceResultSchema.parse({contents:[content]});
}
const malformed = createResources({readServerResource:async () => ReadResourceResultSchema.parse({
  contents:[{uri:"file://a",text:"hello",_meta:{"openai/resource":{etag:null}}}]
})});
const malformedResult = await malformed.read({uri:"file://a"});
if (malformedResult.contents[0].openaiMetadata !== undefined) throw new Error("malformed decoration was retained");
console.log(JSON.stringify({metadataPreservation:true,invalidContentRejected:true,empty,sent,result}));
