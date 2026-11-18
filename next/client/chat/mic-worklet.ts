// Runs in the AudioWorklet: hands the microphone to the page in 20 ms frames (960 samples at 48 kHz).
declare class AudioWorkletProcessor {readonly port:MessagePort}
declare function registerProcessor(name:string,processor:new()=>AudioWorkletProcessor):void;

class MicFrames extends AudioWorkletProcessor {
  private frame=new Float32Array(960);private filled=0;
  process(inputs:Float32Array[][]){
    const input=inputs[0]?.[0];
    if(!input)return true;
    let offset=0;
    while(offset<input.length){
      const take=Math.min(input.length-offset,960-this.filled);
      this.frame.set(input.subarray(offset,offset+take),this.filled);this.filled+=take;offset+=take;
      if(this.filled===960){this.port.postMessage(this.frame,[this.frame.buffer]);this.frame=new Float32Array(960);this.filled=0;}
    }
    return true;
  }
}
registerProcessor("dar-mic",MicFrames);
