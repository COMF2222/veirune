import test from 'node:test';
import assert from 'node:assert/strict';
import { listenerLabel } from '../src/communityLabels.ts';
test('listener counts use Russian and English plural forms',()=>{
  assert.equal(listenerLabel(1),'1 слушатель');
  assert.equal(listenerLabel(4),'4 слушателя');
  assert.equal(listenerLabel(5),'5 слушателей');
  assert.equal(listenerLabel(11),'11 слушателей');
  assert.equal(listenerLabel(22),'22 слушателя');
  assert.equal(listenerLabel(114),'114 слушателей');
  assert.equal(listenerLabel(1,true),'1 listener');
  assert.equal(listenerLabel(2,true),'2 listeners');
});
