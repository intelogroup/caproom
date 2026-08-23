// Streaming-test fixture: prints MARK-A immediately, MARK-B three seconds
// later, then exits. A consumer that relays live sees a ~3s arrival gap;
// one that buffers everything until exit sees ~0ms.
console.log('MARK-A');
setTimeout(function () { console.log('MARK-B'); }, 3000);
