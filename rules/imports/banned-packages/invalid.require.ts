// @path src/legacy.cts
import got = require('got'); // expect: banned-packages

export const client = got.extend({ timeout: { request: 1000 } });
