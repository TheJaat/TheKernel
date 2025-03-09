void kmain() {
    unsigned short* vidMem = (unsigned short*)0xb8000;
    unsigned char attribute = (1 << 4) | 15; // Blue background, white text
    
    for(int i = 0; i <80*25; i++) {
    	vidMem[i] = (attribute << 8) | ' '; // Set each cell to a space with attribute.
    }


    // Infinite loop
    while(1){}
    
}
