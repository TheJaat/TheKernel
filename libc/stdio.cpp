#include <stdio.h>
#include <terminal/terminal.h>
#include <video/video.h>

// Convert an integer to a string
void itoa(int value, char* str_buffer, int base) {
    // Point to the empty buffer
    char* ptr = str_buffer;
    char* ptr1 = str_buffer;

    char tmp_char;
    int tmp_value;

    // Handle the 0 value, explicitly
    if (value == 0) {
        *str_buffer++ = '0';
        *str_buffer = '\0';
        return;
    }

    // Process each digit
    while (value != 0) {
        // get the last digit, by dividing with the base, which could be base 10 (decimal), base 16 (hex)
        tmp_value = value % base;
        *ptr++ = (tmp_value < 10) ? (tmp_value + '0') : (tmp_value - 10 + 'a');
        // remove the last digit from the value.
        value /= base;
    }

    // Append the null terminator to the buffer
    *ptr-- = '\0';

    // Reverse the string
    // ptr1 is pointing to the bufffer before actual processing of number
    // ptr is pointing to the bufffer with the converted value.
    while (ptr1 < ptr) {
        tmp_char = *ptr;
        *ptr = *ptr1;
        *ptr1 = tmp_char;
        ptr--;
        ptr1++;
    }
}

// Convert an unsigned integer to a string
void utoa(unsigned int value, char* str, int base) {
    char* ptr = str;
    char* ptr1 = str;
    char tmp_char;
    unsigned int tmp_value;

    if (value == 0) {
        *str++ = '0';
        *str = '\0';
        return;
    }

    while (value != 0) {
        tmp_value = value % base;
        *ptr++ = (tmp_value < 10) ? (tmp_value + '0') : (tmp_value - 10 + 'a');
        value /= base;
    }

    *ptr-- = '\0';

    while (ptr1 < ptr) {
        tmp_char = *ptr;
        *ptr = *ptr1;
        *ptr1 = tmp_char;
        ptr--;
        ptr1++;
    }
}

// Convert an float to a string
void ftoa(double num, char* str, int precision) {

     int intPart = (int)num;
     double fracPart = num - (double)intPart;
     
     itoa(intPart, str, 10);
     
     while(*str) str++;
     
     if(precision > 0) {
     	*str++ = '.';
     	while (precision--) {
     		fracPart *= 10;
     		int fracDigit = (int) fracPart;
     		*str++ = fracDigit + '0';
     		fracPart -= fracDigit;
     	}
     }
     
     *str = '\0';

}

// Convert an pointer address to a string
void ptoa(uintptr_t value, char* str, int base) {
    char* ptr = str;
    char* ptr1 = str;
    char tmp_char;
    unsigned int tmp_value;

    // Handle 0 explicitly
    if (value == 0) {
        *str++ = '0';
        *str = '\0';
        return;
    }

    // Process each digit
    while (value != 0) {
        tmp_value = value % base;
        *ptr++ = (tmp_value < 10) ? (tmp_value + '0') : (tmp_value - 10 + 'a');
        value /= base;
    }

    // Null-terminate string
    *ptr-- = '\0';


    // Reverse the string
    while (ptr1 < ptr) {
        tmp_char = *ptr;
        *ptr = *ptr1;
        *ptr1 = tmp_char;
        ptr--;
        ptr1++;
    }
}


// vsprintf:
// Formats and stores a string in a buffer based on a format string
// and a va_list of arguments.
int vsprintf(char* buffer, const char* format, va_list args) {

    // Pointer to the buffer to keep track of the current position
    char* buf_ptr = buffer;
    const char* fmt_ptr = format;
    char ch;
    char tmp[32];

    // Iterate through the format string
    while ((ch = *fmt_ptr++) != '\0') {
        if (ch != '%') {
            *buf_ptr++ = ch;
            continue;
        }

        ch = *fmt_ptr++;
        switch (ch) {
            case 'd': { // handle integer
                int value = va_arg(args, int);
                itoa(value, tmp, 10);
                for (char* tmp_ptr = tmp; *tmp_ptr != '\0'; tmp_ptr++) {
                    *buf_ptr++ = *tmp_ptr;
                }
                break;
            }
            case 'u': {  // handle Unsigned Integer
                unsigned int val = va_arg(args, unsigned int);
                utoa(val, tmp, 10);
                // strcpy(buf_ptr, tmp);
                // buf_ptr += strlen(tmp);
                for (char* tmp_ptr = tmp; *tmp_ptr != '\0'; tmp_ptr++) {
                    *buf_ptr++ = *tmp_ptr;
                }
                break;
            }
            case 'x': { // handle hex integer
                int value = va_arg(args, int);
                itoa(value, tmp, 16);
                for (char* tmp_ptr = tmp; *tmp_ptr != '\0'; tmp_ptr++) {
                    *buf_ptr++ = *tmp_ptr;
                }
                break;
            }
            case 's': { // handle string
                char* str = va_arg(args, char*);
                while (*str != '\0') {
                    *buf_ptr++ = *str++;
                }
                break;
            }
            case 'c': { // handle character
                char value = (char)va_arg(args, int);
                *buf_ptr++ = value;
                break;
            }
            case 'p': { // handle pointer
            	uintptr_t ptr = (uintptr_t) va_arg(args, void*);
            	*buf_ptr++ = '0';
            	*buf_ptr++ = 'x';
            	ptoa(ptr, tmp, 16);
            	for (char* tmp_ptr = tmp; *tmp_ptr != '\0'; tmp_ptr++) {
                    *buf_ptr++ = *tmp_ptr;
                }
                break;            	
            }
            case 'f': { // handle floating-point
            	double dbl = va_arg(args, double);
            	ftoa(dbl, tmp, 6);
            	for (char* tmp_ptr = tmp; *tmp_ptr != '\0'; tmp_ptr++) {
                    *buf_ptr++ = *tmp_ptr;
                }
          	break; 
            }
            default: {  // handle unknown format specifier
                *buf_ptr++ = ch;
                break;
            }
        }
    }

    *buf_ptr = '\0';
    return buf_ptr - buffer;
}


void printf(const char* format, ...) {
    va_list args;
    va_start(args, format);

    char buffer[256];
    vsprintf(buffer, format, args);

    va_end(args);

    TerminalDrawString(&BootTerminal, buffer);
}