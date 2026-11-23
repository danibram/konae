#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include "quickjs.h"

#define IMPORT(name) __attribute__((import_module("konae"), import_name(#name)))
#define EXPORT(name) __attribute__((export_name(#name)))

IMPORT(op_worker_call) int op_worker_call(const char *, size_t, char *, size_t);
IMPORT(op_worker_body_read) int op_worker_body_read(uint8_t *, size_t);
IMPORT(op_worker_failure) void op_worker_failure(const char *, size_t);

enum {
    OK, EXECUTION_FAILED, IMPORT_FORBIDDEN, HEAP_EXHAUSTED, MISSING_HANDLE,
    INVALID_RESPONSE, MISSING_SHUTDOWN,
};

static JSContext *ctx;
static JSRuntime *runtime;
static JSValue invoke, shutdown, handler, on_shutdown;
static bool import_denied;
static const char *reply;
static size_t reply_length;

static JSValue call_action(JSContext *context, JSValueConst self, int argc, JSValueConst *args) {
    size_t length;
    const char *request = JS_ToCStringLen(context, &length, args[0]);
    if (!request) return JS_EXCEPTION;
    int count = op_worker_call(request, length, NULL, 0);
    JS_FreeCString(context, request);
    if (count < 0) return JS_ThrowTypeError(context, "Action unavailable");
    char *response = malloc((size_t)count);
    if (!response) return JS_ThrowOutOfMemory(context);
    int copied = op_worker_call(NULL, 0, response, (size_t)count);
    JSValue value = copied == count
        ? JS_NewStringLen(context, response, (size_t)count)
        : JS_ThrowTypeError(context, "Invalid action result");
    free(response);
    return value;
}

static JSValue read_body(JSContext *context, JSValueConst self, int argc, JSValueConst *args) {
    uint8_t bytes[65536];
    int length = op_worker_body_read(bytes, sizeof(bytes));
    if (length < 0) return JS_ThrowTypeError(context, "Body unavailable");
    if (!length) return JS_NULL;
    return JS_NewArrayBufferCopy(context, bytes, (size_t)length);
}

static JSValue fail(JSContext *context, JSValueConst self, int argc, JSValueConst *args) {
    size_t length;
    const char *code = JS_ToCStringLen(context, &length, args[0]);
    if (!code) return JS_EXCEPTION;
    op_worker_failure(code, length);
    JS_FreeCString(context, code);
    return JS_UNDEFINED;
}

static JSModuleDef *deny_import(JSContext *context, const char *name, void *opaque) {
    import_denied = true;
    JS_ThrowReferenceError(context, "function_import_forbidden");
    return NULL;
}

static int classify(JSValue error) {
    int code = EXECUTION_FAILED;
    JSValue message = JS_GetPropertyStr(ctx, error, "message");
    const char *text = JS_ToCString(ctx, message);
    if (text && strcmp("out of memory", text) == 0) code = HEAP_EXHAUSTED;
    JS_FreeCString(ctx, text);
    JS_FreeValue(ctx, message);
    JS_FreeValue(ctx, error);
    return code;
}

static int exception(void) {
    return classify(JS_GetException(ctx));
}

static int settle(JSValue value, JSValue *result) {
    if (JS_IsException(value)) return exception();
    if (JS_PromiseState(ctx, value) == JS_PROMISE_NOT_A_PROMISE) {
        *result = value;
        return OK;
    }
    while (JS_PromiseState(ctx, value) == JS_PROMISE_PENDING) {
        JSContext *job_context = NULL;
        int pending = JS_ExecutePendingJob(runtime, &job_context);
        if (pending <= 0) {
            JS_FreeValue(ctx, value);
            return pending < 0 ? exception() : EXECUTION_FAILED;
        }
    }
    JSPromiseStateEnum state = JS_PromiseState(ctx, value);
    *result = JS_PromiseResult(ctx, value);
    JS_FreeValue(ctx, value);
    return state == JS_PROMISE_FULFILLED ? OK : classify(*result);
}

EXPORT(guest_alloc) void *guest_alloc(size_t size) {
    return calloc(size + 1, 1);
}

static int prepare_runtime(size_t heap) {
    runtime = JS_NewRuntime();
    if (!runtime) return HEAP_EXHAUSTED;
    JS_SetMemoryLimit(runtime, heap);
    JS_SetModuleLoaderFunc(runtime, NULL, deny_import, NULL);
    ctx = JS_NewContext(runtime);
    return ctx ? OK : HEAP_EXHAUSTED;
}

EXPORT(guest_pack) int guest_pack(char *source, size_t length, size_t heap, int module) {
    int code = prepare_runtime(heap);
    if (code != OK) return code;
    JSValue compiled = JS_Eval(ctx, source, length, module ? "function.mjs" : "web.js",
                               (module ? JS_EVAL_TYPE_MODULE : JS_EVAL_TYPE_GLOBAL) |
                               JS_EVAL_FLAG_COMPILE_ONLY);
    free(source);
    if (JS_IsException(compiled)) return import_denied ? IMPORT_FORBIDDEN : exception();
    reply = (const char *)JS_WriteObject(ctx, &reply_length, compiled, JS_WRITE_OBJ_BYTECODE);
    JS_FreeValue(ctx, compiled);
    if (reply) return OK;
    return JS_HasException(ctx) ? exception() : EXECUTION_FAILED;
}

EXPORT(guest_init) int guest_init(char *bootstrap, size_t length, size_t heap) {
    int code = prepare_runtime(heap);
    if (code != OK) return code;
    JSValue global = JS_GetGlobalObject(ctx);
    JS_SetPropertyStr(ctx, global, "op_worker_call", JS_NewCFunction(ctx, call_action, "callAction", 1));
    JS_SetPropertyStr(ctx, global, "op_worker_body_read", JS_NewCFunction(ctx, read_body, "readBody", 0));
    JS_SetPropertyStr(ctx, global, "op_worker_failure", JS_NewCFunction(ctx, fail, "fail", 1));
    JSValue compiled = JS_ReadObject(ctx, (const uint8_t *)bootstrap, length, JS_READ_OBJ_BYTECODE);
    free(bootstrap);
    if (JS_IsException(compiled)) return exception();
    JSValue initialized = JS_EvalFunction(ctx, compiled);
    if (JS_IsException(initialized)) return exception();
    JS_FreeValue(ctx, initialized);
    JSValue api = JS_GetPropertyStr(ctx, global, "__konae");
    invoke = JS_GetPropertyStr(ctx, api, "invoke");
    shutdown = JS_GetPropertyStr(ctx, api, "shutdown");
    JS_SetPropertyStr(ctx, global, "__konae", JS_UNDEFINED);
    JS_FreeValue(ctx, api);
    JS_FreeValue(ctx, global);
    return OK;
}

EXPORT(guest_load) int guest_load(char *bytecode, size_t length) {
    JSValue compiled = JS_ReadObject(ctx, (const uint8_t *)bytecode, length, JS_READ_OBJ_BYTECODE);
    free(bytecode);
    if (JS_IsException(compiled)) return exception();
    if (JS_ResolveModule(ctx, compiled) < 0) {
        JS_FreeValue(ctx, compiled);
        return import_denied ? IMPORT_FORBIDDEN : exception();
    }
    JSModuleDef *module = JS_VALUE_GET_PTR(compiled);
    JSValue result;
    int status = settle(JS_EvalFunction(ctx, compiled), &result);
    if (status != OK) return import_denied ? IMPORT_FORBIDDEN : status;
    JS_FreeValue(ctx, result);
    JSValue exports = JS_GetModuleNamespace(ctx, module);
    handler = JS_GetPropertyStr(ctx, exports, "handle");
    on_shutdown = JS_GetPropertyStr(ctx, exports, "shutdown");
    JS_FreeValue(ctx, exports);
    return JS_IsFunction(ctx, handler) ? OK : MISSING_HANDLE;
}

EXPORT(guest_invoke) int guest_invoke(char *input, size_t length, unsigned response_limit) {
    JSValue args[] = {handler, JS_NewStringLen(ctx, input, length), JS_NewUint32(ctx, response_limit)};
    free(input);
    if (JS_IsException(args[1])) return exception();
    JSValue invocation = JS_Call(ctx, invoke, JS_UNDEFINED, 3, args);
    JS_FreeValue(ctx, args[1]);
    JSValue result;
    int status = settle(invocation, &result);
    if (status != OK) return status;
    reply = JS_ToCStringLen(ctx, &reply_length, result);
    JS_FreeValue(ctx, result);
    return reply ? OK : INVALID_RESPONSE;
}

EXPORT(guest_reply_ptr) const char *guest_reply_ptr(void) { return reply; }
EXPORT(guest_reply_len) size_t guest_reply_len(void) { return reply_length; }

EXPORT(guest_end_request) void guest_end_request(void) {
    JS_FreeCString(ctx, reply);
    reply = NULL;
    reply_length = 0;
}

EXPORT(guest_shutdown) int guest_shutdown(void) {
    if (!JS_IsFunction(ctx, on_shutdown)) return MISSING_SHUTDOWN;
    JSValue result;
    int status = settle(JS_Call(ctx, shutdown, JS_UNDEFINED, 1, &on_shutdown), &result);
    if (status == OK) JS_FreeValue(ctx, result);
    return status;
}

EXPORT(guest_spin) void guest_spin(void) {
    for (;;) __asm__ volatile("");
}
