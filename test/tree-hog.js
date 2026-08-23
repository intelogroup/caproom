// Forks three ~90MB holders. Each process stays well under any sane
// per-process limit; the TREE crosses it. A top-pid-only watchdog never
// fires here — this exists to prove the parent->child walk does.
const { spawn } = require('child_process');

const holder =
  'const a=[];for(let i=0;i<9;i++)a.push(Buffer.alloc(10*1024*1024).fill(1));' +
  'setInterval(function(){},1000);';

for (let i = 0; i < 3; i++) {
  spawn(process.execPath, ['-e', holder], { stdio: 'ignore' });
}
setInterval(function () {}, 1000);
