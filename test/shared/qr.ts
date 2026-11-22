/**
 * A small QR code encoder (byte mode, error correction M) for WhatsApp link codes.
 * Follows ISO/IEC 18004 as laid out in Project Nayuki's reference implementation (MIT).
 * Returns the module grid; true is dark.
 */
const eccPerBlock=[-1,10,16,26,18,24,16,18,22,22,26,30,22,22,24,24,28,28,26,26,26,26,28,28,28,28,28,28,28,28,28,28,28,28,28,28,28,28,28,28,28];
const blockCount=[-1,1,1,1,2,2,4,4,4,5,5,5,8,9,9,10,10,11,13,14,16,17,17,18,20,21,23,25,26,28,29,31,33,35,37,38,40,43,45,47,49];
const formatBitsM=0;

function rawModules(version:number){
  let result=(16*version+128)*version+64;
  if(version>=2){const align=Math.floor(version/7)+2;result-=(25*align-10)*align-55;if(version>=7)result-=36;}
  return result;
}
const dataCodewords=(version:number)=>Math.floor(rawModules(version)/8)-eccPerBlock[version]*blockCount[version];
function multiply(x:number,y:number){let z=0;for(let i=7;i>=0;i--){z=(z<<1)^((z>>>7)*0x11d);z^=((y>>>i)&1)*x;}return z;}
function divisor(degree:number){
  const result=new Array<number>(degree).fill(0);result[degree-1]=1;let root=1;
  for(let i=0;i<degree;i++){for(let j=0;j<degree;j++){result[j]=multiply(result[j],root);if(j+1<degree)result[j]^=result[j+1];}root=multiply(root,2);}
  return result;
}
function remainder(data:number[],div:number[]){
  const result=new Array<number>(div.length).fill(0);
  for(const b of data){const factor=b^result.shift()!;result.push(0);div.forEach((coef,i)=>{result[i]^=multiply(coef,factor);});}
  return result;
}
const bit=(value:number,i:number)=>((value>>>i)&1)!==0;

export function qrMatrix(text:string):boolean[][]{
  const bytes=[...new TextEncoder().encode(text)];
  let version=1;
  for(;version<=40;version++){const countBits=version<10?8:16;if(4+countBits+8*bytes.length<=dataCodewords(version)*8)break;}
  if(version>40)throw new Error("The code is too long for a QR code.");
  // Data bits: byte mode, length, bytes, terminator, padding.
  const bits:number[]=[],push=(value:number,length:number)=>{for(let i=length-1;i>=0;i--)bits.push((value>>>i)&1);};
  push(4,4);push(bytes.length,version<10?8:16);for(const b of bytes)push(b,8);
  const capacity=dataCodewords(version)*8;
  push(0,Math.min(4,capacity-bits.length));push(0,(8-bits.length%8)%8);
  for(let pad=0xec;bits.length<capacity;pad^=0xec^0x11)push(pad,8);
  const data:number[]=[];for(let i=0;i<bits.length;i+=8)data.push(bits.slice(i,i+8).reduce((a,b)=>a<<1|b,0));
  // Error correction, split into blocks and interleaved.
  const blocks=blockCount[version],eccLength=eccPerBlock[version],raw=Math.floor(rawModules(version)/8),short=blocks-raw%blocks,shortLength=Math.floor(raw/blocks),div=divisor(eccLength);
  const parts:number[][]=[];
  for(let i=0,k=0;i<blocks;i++){
    const chunk=data.slice(k,k+shortLength-eccLength+(i<short?0:1));k+=chunk.length;
    const ecc=remainder(chunk,div);if(i<short)chunk.push(0);parts.push([...chunk,...ecc]);
  }
  const codewords:number[]=[];
  for(let i=0;i<parts[0].length;i++)for(let j=0;j<parts.length;j++)if(i!==shortLength-eccLength||j>=short)codewords.push(parts[j][i]);

  const size=version*4+17,modules=Array.from({length:size},()=>new Array<boolean>(size).fill(false)),fixed=Array.from({length:size},()=>new Array<boolean>(size).fill(false));
  const set=(x:number,y:number,dark:boolean)=>{modules[y][x]=dark;fixed[y][x]=true;};
  for(let i=0;i<size;i++){set(6,i,i%2===0);set(i,6,i%2===0);}
  for(const [cx,cy] of [[3,3],[size-4,3],[3,size-4]])for(let dy=-4;dy<=4;dy++)for(let dx=-4;dx<=4;dx++){
    const x=cx+dx,y=cy+dy,distance=Math.max(Math.abs(dx),Math.abs(dy));if(x>=0&&x<size&&y>=0&&y<size)set(x,y,distance!==2&&distance!==4);
  }
  if(version>1){
    const count=Math.floor(version/7)+2,step=version===32?26:Math.ceil((version*4+4)/(count*2-2))*2,positions=[6];
    for(let pos=size-7;positions.length<count;pos-=step)positions.splice(1,0,pos);
    for(let i=0;i<count;i++)for(let j=0;j<count;j++){
      if(i===0&&j===0||i===0&&j===count-1||i===count-1&&j===0)continue;
      for(let dy=-2;dy<=2;dy++)for(let dx=-2;dx<=2;dx++)set(positions[i]+dx,positions[j]+dy,Math.max(Math.abs(dx),Math.abs(dy))!==1);
    }
  }
  const format=(mask:number)=>{
    const value=formatBitsM<<3|mask;let rem=value;for(let i=0;i<10;i++)rem=(rem<<1)^((rem>>>9)*0x537);
    const f=(value<<10|rem)^0x5412;
    for(let i=0;i<=5;i++)set(8,i,bit(f,i));
    set(8,7,bit(f,6));set(8,8,bit(f,7));set(7,8,bit(f,8));
    for(let i=9;i<15;i++)set(14-i,8,bit(f,i));
    for(let i=0;i<8;i++)set(size-1-i,8,bit(f,i));
    for(let i=8;i<15;i++)set(8,size-15+i,bit(f,i));
    set(8,size-8,true);
  };
  format(0);
  if(version>=7){
    let rem=version;for(let i=0;i<12;i++)rem=(rem<<1)^((rem>>>11)*0x1f25);
    const v=version<<12|rem;for(let i=0;i<18;i++){const a=size-11+i%3,b=Math.floor(i/3);set(a,b,bit(v,i));set(b,a,bit(v,i));}
  }
  let i=0;
  for(let right=size-1;right>=1;right-=2){
    if(right===6)right=5;
    for(let vert=0;vert<size;vert++)for(let j=0;j<2;j++){
      const x=right-j,y=((right+1)&2)===0?size-1-vert:vert;
      if(!fixed[y][x]&&i<codewords.length*8){modules[y][x]=bit(codewords[i>>>3],7-(i&7));i++;}
    }
  }
  const masks:((x:number,y:number)=>boolean)[]=[(x,y)=>(x+y)%2===0,(_x,y)=>y%2===0,x=>x%3===0,(x,y)=>(x+y)%3===0,(x,y)=>(Math.floor(x/3)+Math.floor(y/2))%2===0,
    (x,y)=>x*y%2+x*y%3===0,(x,y)=>(x*y%2+x*y%3)%2===0,(x,y)=>((x+y)%2+x*y%3)%2===0];
  const apply=(mask:number)=>{for(let y=0;y<size;y++)for(let x=0;x<size;x++)if(!fixed[y][x]&&masks[mask](x,y))modules[y][x]=!modules[y][x];};
  // Pick the mask with the fewest long runs and 2×2 blocks and the best balance.
  const penalty=()=>{
    let score=0,dark=0;
    for(let a=0;a<size;a++){
      let rowRun=1,colRun=1;
      for(let b=1;b<size;b++){
        if(modules[a][b]===modules[a][b-1]){rowRun++;if(rowRun===5)score+=3;else if(rowRun>5)score++;}else rowRun=1;
        if(modules[b][a]===modules[b-1][a]){colRun++;if(colRun===5)score+=3;else if(colRun>5)score++;}else colRun=1;
      }
    }
    for(let y=0;y<size;y++)for(let x=0;x<size;x++){
      if(modules[y][x])dark++;
      if(x<size-1&&y<size-1&&modules[y][x]===modules[y][x+1]&&modules[y][x]===modules[y+1][x]&&modules[y][x]===modules[y+1][x+1])score+=3;
    }
    return score+Math.floor(Math.abs(dark*20-size*size*10)/(size*size))*10;
  };
  let best=0,bestScore=Infinity;
  for(let mask=0;mask<8;mask++){apply(mask);format(mask);const score=penalty();if(score<bestScore){best=mask;bestScore=score;}apply(mask);}
  apply(best);format(best);
  return modules;
}

/** An SVG path of the dark modules, offset by a four-module quiet zone. */
export function qrPath(matrix:boolean[][]){
  let d="";
  matrix.forEach((row,y)=>row.forEach((dark,x)=>{if(dark)d+=`M${x+4} ${y+4}h1v1h-1z`;}));
  return {d,size:matrix.length+8};
}
