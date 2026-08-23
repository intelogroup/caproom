// Child IGNORES SIGTERM and holds ~200MB while its parent stays small.
// Proves the escalation path: root dies during grace, the breach-time pid
// snapshot still finds the stubborn child and SIGKILLs it — no orphan left
// allocating past the cap after caproom exits. Expects exit 137.
const { spawn } = require('child_process');

const holder =
  'process.on("SIGTERM",function(){});' +
  'const a=[];for(let i=0;i<20;i++)a.push(Buffer.alloc(10*1024*1024).fill(1));' +
  'setInterval(function(){},1000);';

spawn(process.execPath, ['-e', holder], { stdio: 'ignore' });
setInterval(function () {}, 1000);
