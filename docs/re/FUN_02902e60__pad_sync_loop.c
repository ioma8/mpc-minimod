
void FUN_02902e60(long param_1,long param_2,int param_3)

{
  long lVar1;
  long lVar2;
  long lVar3;
  
  if (param_3 == 0) {
    lVar3 = 0;
    do {
      lVar1 = *(long *)(param_1 + 0x22e8) + lVar3;
      lVar2 = *(long *)(param_2 + 0x22e8) + lVar3;
      lVar3 = lVar3 + 0xb30;
      FUN_02918be0(lVar1,lVar2);
    } while (lVar3 != 0x5980);
  }
  else if (param_3 == 1) {
    lVar3 = 0xb30;
    do {
      lVar2 = lVar3 + -0xb30;
      lVar1 = *(long *)(param_1 + 0x22e8) + lVar3;
      lVar3 = lVar3 + 0xb30;
      FUN_02918be0(lVar1,*(long *)(param_2 + 0x22e8) + lVar2);
    } while (lVar3 != 0x5980);
  }
  return;
}

