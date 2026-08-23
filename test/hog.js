let a = [];
let i = 0;
setInterval(() => {
  a.push(Buffer.alloc(10 * 1024 * 1024).fill(1));
  i++;
  console.log('allocated', i * 10, 'MB');
}, 100);
