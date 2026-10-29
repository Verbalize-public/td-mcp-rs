uniform vec4 uMotion;
out vec4 fragColor;
void main() {
 vec2 uv=vUV.st; vec2 p=(uv-0.5)*vec2(1.77778,1.0);
 float t=uMotion.x;
 vec3 col=vec3(0.012,0.02,0.045);
 for(int i=0;i<28;i++){
  float k=float(i)/27.0;
  float y=0.24*sin(p.x*3.4+t*0.8+k*2.8)+0.12*cos(p.x*6.0-t*0.45+k*1.6)+(k-0.5)*0.38;
  float d=abs(p.y-y);
  float line=exp(-d*d/0.000018);
  float glow=exp(-d*d/0.0018)*0.065;
  vec3 ink=mix(vec3(0.05,0.8,0.95),vec3(1.0,0.18,0.45),k);
  ink=mix(ink,vec3(1.0,0.67,0.22),smoothstep(0.7,1.0,k));
  col+=ink*(line*0.8+glow);
 }
 col*=1.0-0.22*length(p);
 fragColor=TDOutputSwizzle(vec4(col,1.0));
}
