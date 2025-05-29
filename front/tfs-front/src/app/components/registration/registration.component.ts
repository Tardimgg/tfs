import { Component } from '@angular/core';
import {AuthService} from "../../services/auth.service";
import {Location} from "@angular/common";
import {FormsModule} from '@angular/forms';

@Component({
  selector: 'app-registration',
  templateUrl: './registration.component.html',
  styleUrls: ['./registration.component.css'],
  imports: [FormsModule]
})
export class RegistrationComponent {

  loginForm = {
    login: '',
    email: '',
    password: '',
  }

  text_error: string = ""

  isLoginMode = true;

  refresh() {
    this.text_error = ""
  }



  switchMode(toRegistration: boolean){
    this.isLoginMode = !toRegistration;
  }

  action(){
    if (this.isLoginMode) {
      this.service.login(this.loginForm.login, this.loginForm.password)
        .subscribe(response => {
          // if (response == "ok") {
          this.service.saveToken(response.access_token);
          // this.service.saveRole(response.roles[0]);
          this.service.saveUserId(0)
          this.location.back();
          this.service.emitLogin(this.loginForm.login);
          // this.service.emitLogin(this.loginForm.login);
          // } else {
          //   this.text_error = response.error;
          // }
        })
    } else {
      this.service.registration(this.loginForm.login, this.loginForm.password, this.loginForm.email)
        .subscribe(response => {
          // if (response == "ok") {
          this.service.saveToken(response.token.access_token);
          // this.service.saveRole(response.roles[0]);
          this.service.saveUserId(response.uid)
          this.location.back();
          this.service.emitLogin(this.loginForm.login);
          // } else {
          //   this.text_error = response.error;
          // }
        })
    }
  }

  constructor(private service: AuthService, private location: Location) {
  }

}
